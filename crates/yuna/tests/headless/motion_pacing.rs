//! Motion's pacing when drawing takes time
//! ([ADR-0056](../../../../docs/adr/0056-motions-budget-judged-on-the-gpu-against-a-still-baseline.md)):
//! on the manual clock, a draw hook passes the time each drawing takes. A refresh is due a lead
//! before the boundary it aims at, samples the motion there, and its frame is presented there if
//! its drawing ended by then, else at the first boundary after
//! ([ADR-0031](../../../../docs/adr/0031-frames-drawn-ahead-of-their-boundary.md),
//! [ADR-0054](../../../../docs/adr/0054-motion-sampled-at-presentation-drawn-apart-from-told.md)).
//!
//! Each story scrolls three full-width columns, the focus moved one column left, under a
//! floating window that every frame shows: the carrier. Before each refresh, the carrier asks a
//! frame callback, which tells the story that the refresh ran, and presentation feedback, which
//! tells where its frame was presented. Every frame is also compared with an independent
//! `Motion`, fed the same layout at the same time as Yuna's, at the boundary the frame sampled.
//! The output is 1920 × 1080 at 240 Hz and scale 1, with `motion.rs`'s animations.
//!
//! The carrier's commits would arm a refresh if Yuna had not: these stories check pacing, not
//! that a motion arms its own refreshes, which `motion.rs` checks.

use std::time::Duration;

use yuna::yuna_layout::Direction;
use yuna::yuna_motion::{Change, Drawn, Motion};
use yuna::{ClockMode, Compositor, Config, OWNER_SEAT, OutputMode};

use crate::client::{Client, Feedback};
use crate::motion::{
    GREEN, animations, arrangement, as_drawn, full_hd, open, open_full_width_columns,
    operate_on_idle, settle, window_id,
};
use crate::{
    BLUE, LEAD, OUTPUT, PERIOD, RED, advance_to, assert_rests, boundary, first_boundary_at_or_after,
};

/// The carrier: the client's first window, floating over the columns.
const CARRIER: usize = 0;

/// What the lead keeps beyond the longest recent drawing (ADR-0031).
const MARGIN: Duration = Duration::from_millis(1);

/// One refresh of a scroll, as a story saw it.
#[derive(Debug)]
struct Refresh {
    /// When it was due.
    due: Duration,
    /// Whether it drew: something on the output changed.
    drew: bool,
    /// What its frame drew, as [`Compositor::drawn`] gave it.
    drawn: Vec<Drawn>,
    /// The number of the refresh its frame was presented at, as its feedback said.
    seq: u64,
    /// When that refresh began, as its feedback said.
    presented: Duration,
}

/// A scroll, run to its end.
struct Scroll {
    compositor: Compositor,
    /// The output's creation: its boundary 0.
    start: Duration,
    /// The reference, updated at the operation, as Yuna's motion was.
    reference: Motion,
    refreshes: Vec<Refresh>,
}

impl Scroll {
    /// The boundary a refresh due at `due` aims at, and samples: [`aimed`].
    fn aimed(&self, due: Duration) -> u64 {
        aimed(self.start, due)
    }

    /// What the reference draws at boundary `k`.
    fn reference_at(&self, k: u64) -> Vec<Drawn> {
        self.reference
            .frame(OUTPUT, boundary(self.start, k))
            .windows
    }

    /// The first boundary at or after the reference's end: where a scroll whose drawings take
    /// no time samples its last frame.
    fn end_boundary(&self, operation: Duration) -> u64 {
        let end = self
            .reference
            .end(OUTPUT, operation)
            .expect("the scroll moves");
        first_boundary_at_or_after(self.start, end)
    }
}

/// A manual-clock compositor of one output of `mode` on the owner's seat, animations on; a
/// client; three full-width columns, the last focused, under the carrier, which floats at the
/// size it had as a column. Returns them and the output's creation.
fn columns_under_the_carrier(mode: OutputMode) -> (Compositor, Client, Duration) {
    let mut compositor = Compositor::new(Config {
        outputs: vec![mode],
        clock: ClockMode::Manual,
        listen: false,
        animations: animations(),
        ..Config::default()
    })
    .expect("the compositor starts");
    let mut client = Client::connect(&mut compositor);
    let start = compositor.now();
    let carrier = open(&mut compositor, &mut client, GREEN);
    assert_eq!(carrier, CARRIER);
    compositor
        .change_layout(|layout| layout.set_floating(window_id(CARRIER), true))
        .expect("the carrier floats");
    settle(&mut compositor, |compositor| client.roundtrip(compositor));
    open_full_width_columns(&mut compositor, &mut client, &[RED, BLUE, RED]);
    (compositor, client, start)
}

/// Scrolls one column left on an output of `mode`, the operation made on the idle output
/// ([`operate_on_idle`]), each drawing from then on taking `drawing(n)`, n counting them from 0;
/// records every refresh ([`record`]) until the one that samples the reference's motion at or
/// after its end, the sample of a refresh due at `due` being `sample(start, due)`. Returns the
/// scroll and the operation's time.
fn scroll(
    mode: OutputMode,
    mut drawing: impl FnMut(u64) -> Duration + 'static,
    sample: impl Fn(Duration, Duration) -> Duration,
) -> (Scroll, Duration) {
    let (mut compositor, mut client, start) = columns_under_the_carrier(mode);
    let mut reference = Motion::new(animations());
    reference.update(compositor.layout(), compositor.now(), Change::Instant);
    let mut drawings = 0;
    compositor.set_draw_hook(move |clock| {
        clock.pass(drawing(drawings));
        drawings += 1;
    });
    let operation = operate_on_idle(
        &mut compositor,
        &mut client,
        &mut reference,
        start,
        |layout| {
            layout
                .focus(OWNER_SEAT, Direction::Left)
                .expect("the focus moves left");
        },
    );
    let refreshes = record(&mut compositor, &mut client, |due| {
        reference.end(OUTPUT, sample(start, due)).is_none()
    });
    client.roundtrip(&mut compositor);
    let scroll = Scroll {
        compositor,
        start,
        reference,
        refreshes,
    };
    (scroll, operation)
}

/// Runs the refreshes of a motion just begun, one at a time, until `is_last(due)` says the one
/// due then was its last, then runs what is left armed, so that every frame's feedback comes.
/// Before each refresh, the carrier asks a frame callback, which comes at the refresh, and
/// presentation feedback, which the refresh takes; the round that reads them runs a refresh due
/// at once, else deadlines run one time at a time until the callback comes.
fn record(
    compositor: &mut Compositor,
    client: &mut Client,
    mut is_last: impl FnMut(Duration) -> bool,
) -> Vec<Refresh> {
    let asked_before = client.window(CARRIER).feedback.len();
    let mut seen = Vec::new();
    loop {
        let called = client.window(CARRIER).frames.len();
        client.request_frame(CARRIER);
        client.request_feedback(CARRIER);
        client.commit(CARRIER);
        let mut due = compositor.now();
        let mut drawn_before = compositor.counters().frames_drawn;
        client.roundtrip(compositor);
        while client.window(CARRIER).frames.len() == called {
            due = compositor
                .next_deadline()
                .expect("a refresh comes while the motion runs");
            drawn_before = compositor.counters().frames_drawn;
            advance_to(compositor, client, due);
        }
        assert_eq!(client.window(CARRIER).frames.len(), called + 1);
        let drew = compositor.counters().frames_drawn > drawn_before;
        seen.push((due, drew, compositor.drawn(OUTPUT).to_vec()));
        if is_last(due) {
            break;
        }
        assert!(seen.len() < 1_000, "the motion never ends");
    }
    let until = compositor.now() + Duration::from_secs(1);
    while let Some(at) = compositor.next_deadline() {
        assert!(at <= until, "the compositor never rests");
        advance_to(compositor, client, at);
    }
    let feedback = &client.window(CARRIER).feedback[asked_before..];
    assert_eq!(feedback.len(), seen.len(), "{feedback:?}");
    seen.into_iter()
        .zip(feedback)
        .map(|((due, drew, drawn), feedback)| {
            let Feedback::Presented { seq, time, .. } = *feedback else {
                panic!("the frame of the refresh due at {due:?} was discarded");
            };
            Refresh {
                due,
                drew,
                drawn,
                seq,
                presented: time,
            }
        })
        .collect()
}

/// The boundary a refresh due at `due` aims at, on a fixed-rate output created at `start`: the
/// first after it. A refresh is due its lead before the boundary it aims at, and the lead is at
/// most one period (ADR-0031); one due at once on the idle output, 0.1 ms after a boundary,
/// reaches the next with its drawing.
fn aimed(start: Duration, due: Duration) -> u64 {
    first_boundary_at_or_after(start, due + Duration::from_nanos(1))
}

/// The sample of a refresh due at `due` on a fixed-rate output created at `start`: the boundary
/// it aims at.
fn boundary_after(start: Duration, due: Duration) -> Duration {
    boundary(start, aimed(start, due))
}

/// The scroll's last frame is the arrangement, and then nothing runs.
fn ends_at_the_arrangement_and_rests(scroll: &mut Scroll) {
    let last = scroll.refreshes.last().expect("a refresh");
    assert_eq!(
        as_drawn(&last.drawn),
        arrangement(&scroll.compositor, OUTPUT)
    );
    assert_eq!(
        as_drawn(scroll.compositor.drawn(OUTPUT)),
        arrangement(&scroll.compositor, OUTPUT)
    );
    assert_rests(&mut scroll.compositor);
}

/// Drawings of 70 % of a period, 2.92 ms: from the first, the lead is the drawing and the
/// margin, 3.92 ms. A scroll of more than 60 frames presents every frame at the boundary it
/// aimed at and sampled, the first after its refresh: the feedback's numbers are consecutive,
/// none missed. The first refresh is due at once, at the operation; every later one is due the
/// lead before its boundary. Each frame is the reference's at its boundary; the last is the
/// arrangement; then nothing runs.
#[test]
fn drawings_of_seventy_percent_of_a_period_miss_no_boundary() {
    let drawing = PERIOD * 7 / 10;
    let (mut scroll, operation) = scroll(full_hd(), move |_| drawing, boundary_after);
    let refreshes = &scroll.refreshes;
    assert!(refreshes.len() > 60, "{} frames", refreshes.len());
    assert_eq!(refreshes[0].due, operation);
    for (n, refresh) in refreshes.iter().enumerate() {
        let k = scroll.aimed(refresh.due);
        assert_eq!(
            (refresh.seq, refresh.presented),
            (k, boundary(scroll.start, k)),
            "frame {n}"
        );
        assert_eq!(refresh.drawn, scroll.reference_at(k), "frame {n}");
        if n > 0 {
            assert_eq!(
                refresh.due,
                boundary(scroll.start, k) - (drawing + MARGIN),
                "frame {n}"
            );
        }
    }
    assert!(
        refreshes
            .windows(2)
            .all(|pair| pair[1].seq == pair[0].seq + 1)
    );
    assert!(refreshes.iter().filter(|refresh| refresh.drew).count() > 60);
    ends_at_the_arrangement_and_rests(&mut scroll);
}

/// The 20th drawing of a scroll takes 1.5 periods, the others none. That frame, sampled at the
/// boundary it aimed at, is presented at the next: exactly one boundary is missed, and no frame
/// shows a sample older than its presentation by more than one period. The slow drawing makes
/// the lead one period: the next refresh is due a period before the boundary after the late
/// frame's, and aims at, samples and is presented at it. Each frame is the reference's at the
/// boundary it sampled. The motion ends at the boundary where the same scroll ends with no slow
/// drawing, at the arrangement; then nothing runs.
#[test]
fn one_drawing_of_a_period_and_a_half_misses_one_boundary_and_ends_on_time() {
    let (baseline, baseline_operation) = scroll(full_hd(), |_| Duration::ZERO, boundary_after);
    let slow = PERIOD * 3 / 2;
    let (mut scroll, _) = scroll(
        full_hd(),
        move |n| if n == 19 { slow } else { Duration::ZERO },
        boundary_after,
    );
    let refreshes = &scroll.refreshes;
    let samples: Vec<u64> = refreshes
        .iter()
        .map(|refresh| scroll.aimed(refresh.due))
        .collect();
    for (n, (refresh, &k)) in refreshes.iter().zip(&samples).enumerate() {
        assert_eq!(refresh.drawn, scroll.reference_at(k), "frame {n}");
        assert_eq!(refresh.presented, boundary(scroll.start, refresh.seq));
        assert!(
            (k..=k + 1).contains(&refresh.seq),
            "frame {n}, sampled at {k}, presented at {}",
            refresh.seq
        );
    }
    let late: Vec<usize> = (0..refreshes.len())
        .filter(|&n| refreshes[n].seq != samples[n])
        .collect();
    assert_eq!(late.len(), 1, "{late:?}");
    let late = late[0];
    assert!(refreshes[late].drew);
    assert!(late > 10 && late + 10 < refreshes.len(), "frame {late}");
    assert_eq!(refreshes[late].seq, samples[late] + 1);
    // The lead is one period: due a period before the boundary after the late frame's.
    let next = &refreshes[late + 1];
    assert_eq!(samples[late + 1], refreshes[late].seq + 1);
    assert_eq!(next.due, boundary(scroll.start, samples[late + 1]) - PERIOD);
    assert_eq!(next.seq, samples[late + 1]);
    let gaps: Vec<u64> = refreshes
        .windows(2)
        .map(|pair| pair[1].seq - pair[0].seq)
        .collect();
    assert_eq!(gaps.iter().filter(|&&gap| gap == 2).count(), 1, "{gaps:?}");
    assert!(gaps.iter().all(|&gap| gap == 1 || gap == 2), "{gaps:?}");

    let last = refreshes.last().expect("a refresh").seq;
    let baseline_last = baseline.refreshes.last().expect("a refresh").seq;
    assert_eq!(baseline_last, baseline.end_boundary(baseline_operation));
    assert_eq!(last, baseline_last);
    ends_at_the_arrangement_and_rests(&mut scroll);
}

/// Every drawing takes 1.2 periods, 5 ms: past the boundary it aims at. Each frame drawn is
/// presented at the boundary after the one it aimed at, and the lead, capped at one period, has
/// the next refresh due a period before the boundary after that one, aiming at it: frames are
/// presented every other boundary. Each is sampled at the boundary it aimed at, one period
/// before it shows, rather than where it is presented: a refresh samples where it aims
/// (ADR-0054), and it cannot aim more than a period ahead (ADR-0031), so the late frames keep
/// even steps of two periods, each shown one period late. A refresh that finds nothing changed
/// draws nothing and is presented where it aimed. The motion ends on time: its last frame, the
/// arrangement, samples the first boundary it aims at at or after the reference's end, at most
/// one after the one a scroll whose drawings take no time ends at; then nothing runs.
#[test]
fn drawings_of_a_period_and_a_fifth_show_every_other_boundary_and_end_on_time() {
    let (mut scroll, operation) = scroll(full_hd(), |_| PERIOD * 6 / 5, boundary_after);
    let refreshes = &scroll.refreshes;
    let samples: Vec<u64> = refreshes
        .iter()
        .map(|refresh| scroll.aimed(refresh.due))
        .collect();
    for (n, (refresh, &k)) in refreshes.iter().zip(&samples).enumerate() {
        assert_eq!(refresh.drawn, scroll.reference_at(k), "frame {n}");
        assert_eq!(refresh.presented, boundary(scroll.start, refresh.seq));
        assert_eq!(refresh.seq, k + u64::from(refresh.drew), "frame {n}");
    }
    for (n, pair) in refreshes.windows(2).enumerate() {
        assert_eq!(samples[n + 1], pair[0].seq + 1, "frame {}", n + 1);
        assert_eq!(
            pair[1].due,
            boundary(scroll.start, samples[n + 1]) - PERIOD,
            "frame {}",
            n + 1
        );
    }
    let drew = refreshes.iter().filter(|refresh| refresh.drew).count();
    assert!(drew > 30, "{drew} frames drawn");
    let end = scroll.end_boundary(operation);
    let last = *samples.last().expect("a refresh");
    assert!(last == end || last == end + 1, "{last} against {end}");
    ends_at_the_arrangement_and_rests(&mut scroll);
}

/// 1/240 s, a VRR output's shortest refresh, to the nanosecond below.
const SHORTEST: Duration = PERIOD;

/// The vertical blank of `vrr.rs`'s panel, the least time between a frame ready and its showing:
/// 269 of 2,429 lines of the shortest refresh.
const BLANK: Duration = Duration::from_nanos(461_438);

/// A VRR output of 48 to 240 Hz, as `vrr.rs` builds it, drawings taking no time. The scroll's
/// first frame, drawn at once on the idle output, is shown a vertical blank later and samples
/// the motion there, its drawing predicted to take no time. From then on, while the motion runs, a refresh is due every shortest
/// refresh, 1/240 s, a lead before its frame is shown, and samples the motion there; frames are
/// shown every shortest refresh, with consecutive refresh numbers, whether they drew or found
/// nothing changed in the motion's last sub-pixel steps. Each frame is the reference's where it
/// sampled; the last is the arrangement; then nothing runs.
#[test]
fn on_a_vrr_output_a_scroll_refreshes_at_the_shortest_refresh() {
    let mode = full_hd().with_vrr(48.0).expect("a VRR range");
    let (mut scroll, operation) = scroll(mode, |_| Duration::ZERO, |_, due| due + LEAD);
    let refreshes = &scroll.refreshes;
    assert!(refreshes.len() > 60, "{} frames", refreshes.len());
    let first = &refreshes[0];
    assert_eq!((first.due, first.presented), (operation, operation + BLANK));
    assert_eq!(
        first.drawn,
        scroll.reference.frame(OUTPUT, first.presented).windows
    );
    for (n, refresh) in refreshes.iter().enumerate().skip(1) {
        assert_eq!(refresh.presented, refresh.due + LEAD, "frame {n}");
        let expected = scroll.reference.frame(OUTPUT, refresh.presented).windows;
        assert_eq!(refresh.drawn, expected, "frame {n}");
    }
    for (n, pair) in refreshes.windows(2).enumerate() {
        let (before, after) = (&pair[0], &pair[1]);
        assert_eq!(after.seq, before.seq + 1, "frame {}", n + 1);
        assert_eq!(
            after.presented - before.presented,
            SHORTEST,
            "frame {}",
            n + 1
        );
        if n > 0 {
            assert_eq!(after.due - before.due, SHORTEST, "frame {}", n + 1);
        }
    }
    assert!(
        refreshes.iter().any(|refresh| !refresh.drew),
        "no refresh found nothing changed"
    );
    ends_at_the_arrangement_and_rests(&mut scroll);
}

/// A VRR scroll with no carrier: nothing but the motion arms its refreshes. The first is drawn
/// at once; from the third on, each is due one shortest refresh after the one before; from the
/// second on, each draws the reference's frame a lead after it was due; some find nothing changed, in the motion's last sub-pixel steps; the motion
/// ends at the arrangement, then nothing runs.
#[test]
fn on_a_vrr_output_a_motion_alone_arms_a_refresh_each_shortest_refresh() {
    let mode = full_hd().with_vrr(48.0).expect("a VRR range");
    let mut compositor = Compositor::new(Config {
        outputs: vec![mode],
        clock: ClockMode::Manual,
        listen: false,
        animations: animations(),
        ..Config::default()
    })
    .expect("the compositor starts");
    let mut client = Client::connect(&mut compositor);
    let start = compositor.now();
    open_full_width_columns(&mut compositor, &mut client, &[RED, BLUE, RED]);
    let mut reference = Motion::new(animations());
    reference.update(compositor.layout(), compositor.now(), Change::Instant);
    operate_on_idle(&mut compositor, &mut client, &mut reference, start, |l| {
        l.focus(OWNER_SEAT, Direction::Left).expect("left");
    });
    let mut dues: Vec<Duration> = Vec::new();
    let mut unchanged = 0;
    loop {
        let due = compositor
            .next_deadline()
            .expect("a refresh while the motion runs");
        let before = compositor.counters().frames_drawn;
        advance_to(&mut compositor, &mut client, due);
        if compositor.counters().frames_drawn == before {
            unchanged += 1;
        }
        if dues.len() >= 2 {
            let gap = due - dues[dues.len() - 1];
            assert_eq!(gap, PERIOD, "refresh {} after the one before", dues.len());
        }
        if !dues.is_empty() {
            assert_eq!(
                compositor.drawn(OUTPUT),
                reference.frame(OUTPUT, due + LEAD).windows.as_slice(),
                "refresh {}",
                dues.len()
            );
        }
        dues.push(due);
        if dues.len() > 1 && reference.end(OUTPUT, due + LEAD).is_none() {
            break;
        }
        assert!(dues.len() < 1_000, "the motion never ends");
    }
    assert!(
        unchanged > 0,
        "no refresh of the motion's tail found nothing changed"
    );
    assert_eq!(
        as_drawn(compositor.drawn(OUTPUT)),
        arrangement(&compositor, OUTPUT)
    );
    client.roundtrip(&mut compositor);
    assert_rests(&mut compositor);
}
