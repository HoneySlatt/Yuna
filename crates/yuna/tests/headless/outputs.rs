//! Outputs created, removed and given another mode while clients run, as the MCP server's
//! `outputs` tool does (M1.5, M1.11), and what the compositor says of its windows
//! (`Compositor::windows`).

use std::time::Duration;

use yuna::{
    CaptureSize, CaptureTarget, ClockMode, Compositor, Config, Error, MAX_OUTPUTS, OWNER_SEAT,
    OutputMode, RETIRED_GLOBAL_LIFETIME,
};
use yuna_ipc::Command;
use yuna_types::{OutputId, WindowId};

use crate::client::{Client, Feedback, Kind};
use crate::{
    BLUE, LEAD, OUTPUT, RED, advance_to, manual_compositor, open_and_commit, owner,
    run_armed_deadlines,
};

/// 1 s / 60, to the nanosecond below.
const PERIOD_AT_60: Duration = Duration::from_nanos(16_666_666);

pub(crate) fn full_hd() -> OutputMode {
    OutputMode::new(1920, 1080, 60.0, 1.0).expect("a valid mode")
}

/// An output created while a client runs is announced to it, and windows open on it once it
/// has the focus. Removed, it is taken back from the client, the feedback it held for a frame
/// not yet at its boundary is discarded, and its window leaves it. Its workspace goes to the
/// other output, which keeps showing its own (ADR-0018): the window is hidden there. The last
/// output is never removed.
#[test]
fn a_removed_output_gives_its_windows_to_another_and_discards_its_feedback() {
    let mut compositor = manual_compositor();
    let mut client = Client::connect(&mut compositor);
    let first = open_and_commit(&mut compositor, &mut client, RED);
    run_armed_deadlines(&mut compositor);
    client.roundtrip(&mut compositor);
    let one = client.output_named("HEADLESS-1").expect("the first output");
    assert_eq!(client.window(first).on_outputs, vec![one]);

    let created_at = compositor.now();
    let second_output = compositor
        .add_output(OWNER_SEAT, full_hd())
        .expect("an output is added");
    assert_eq!(second_output, OutputId(2));
    // Once for the global, once for the events of the output bound.
    client.roundtrip(&mut compositor);
    client.roundtrip(&mut compositor);
    let two = client
        .output_named("HEADLESS-2")
        .expect("the new output is announced");
    compositor
        .command(owner(), Command::FocusOutput(second_output))
        .expect("the new output takes the focus");
    let second = open_and_commit(&mut compositor, &mut client, BLUE);

    // Output 2's first refresh after its creation is due a lead before its boundary 1: its
    // frame is drawn, and the feedback waits for the boundary.
    let deadline = created_at + PERIOD_AT_60 - LEAD;
    compositor
        .advance(deadline.saturating_sub(compositor.now()))
        .expect("the manual clock moves");
    client.roundtrip(&mut compositor);
    assert_eq!(client.window(second).on_outputs, vec![two]);
    assert_eq!(client.window(second).feedback, vec![]);
    let windows = compositor.windows();
    assert_eq!(windows[1].id, WindowId(2));
    assert_eq!(windows[1].output, Some(second_output));

    compositor
        .remove_output(second_output)
        .expect("the output is removed");
    client.roundtrip(&mut compositor);
    assert_eq!(client.state.removed_globals, vec![two]);
    assert_eq!(client.window(second).feedback, vec![Feedback::Discarded]);
    assert_eq!(client.window(second).on_outputs, Vec::<u32>::new());
    // Its workspace went to output 1, which keeps showing its own: the window is hidden there.
    run_armed_deadlines(&mut compositor);
    client.roundtrip(&mut compositor);
    assert_eq!(client.window(second).on_outputs, Vec::<u32>::new());
    assert_eq!(client.window(first).on_outputs, vec![one]);
    let windows = compositor.windows();
    assert_eq!(
        windows
            .iter()
            .map(|w| (w.output, w.rect.is_some()))
            .collect::<Vec<_>>(),
        vec![(Some(OUTPUT), true), (None, false)]
    );
    let moved = compositor
        .layout()
        .window(WindowId(2))
        .expect("the second window");
    assert_eq!(moved.output, Some(OUTPUT));
    assert_eq!(compositor.outputs(), vec![OUTPUT]);
    compositor
        .check_windows()
        .expect("the layout and the shell agree");
    compositor
        .check_popups_and_keyboard()
        .expect("the keyboard is sound");

    assert!(matches!(
        compositor.remove_output(second_output),
        Err(Error::Refused(_))
    ));
    assert!(matches!(
        compositor.remove_output(OUTPUT),
        Err(Error::Refused(_))
    ));
    assert_eq!(compositor.outputs(), vec![OUTPUT]);
}

/// A window is described as an agent reads it: its application id and title, its client's
/// process, where it is drawn in logical units, and whether it has the focus.
#[test]
fn windows_are_described_with_their_ids_titles_places_and_focus() {
    let mut compositor = manual_compositor();
    let mut client = Client::connect(&mut compositor);
    assert_eq!(compositor.windows(), vec![]);
    let window = client.create_window();
    client.set_app_id_and_title(window, "org.example.Test", "A title");
    client.commit(window);
    client.roundtrip(&mut compositor);
    client.ack(window);
    client.attach(window, crate::TILE.0, crate::TILE.1, RED);
    client.commit(window);
    client.roundtrip(&mut compositor);
    run_armed_deadlines(&mut compositor);
    let windows = compositor.windows();
    assert_eq!(windows.len(), 1);
    let described = &windows[0];
    assert_eq!(described.id, WindowId(1));
    assert_eq!(described.app_id.as_deref(), Some("org.example.Test"));
    assert_eq!(described.title.as_deref(), Some("A title"));
    // The test's clients speak from the test's own process.
    assert_eq!(described.pid, Some(std::process::id() as i32));
    assert_eq!(described.output, Some(OUTPUT));
    let rect = described.rect.expect("the window is shown");
    assert_eq!(
        (rect.x, rect.y, rect.w, rect.h),
        (16.0, 16.0, 1256.0, 1408.0)
    );
    assert!(described.focused);
    assert!(!described.floating && !described.fullscreen);
}

/// A compositor keeps 16 outputs at most: the next is refused, and nothing of it is made.
#[test]
fn outputs_past_the_cap_are_refused() {
    let mut compositor = manual_compositor();
    let small = OutputMode::new(64, 64, 60.0, 1.0).expect("a valid mode");
    for _ in 1..MAX_OUTPUTS {
        compositor
            .add_output(OWNER_SEAT, small)
            .expect("an output under the cap");
    }
    assert!(matches!(
        compositor.add_output(OWNER_SEAT, small),
        Err(Error::Refused(_))
    ));
    assert_eq!(compositor.outputs().len(), MAX_OUTPUTS);
    compositor
        .remove_output(OutputId(2))
        .expect("an output removed");
    assert_eq!(
        compositor
            .add_output(OWNER_SEAT, small)
            .expect("room again"),
        OutputId(MAX_OUTPUTS as u64 + 1)
    );
}

/// A removed output's global stays, disabled, for [`RETIRED_GLOBAL_LIFETIME`] of Yuna's clock,
/// then is destroyed: a client that binds it before it reads the removal is answered until then,
/// and ended for it after. On the manual clock that lifetime is the time a test steps, whatever
/// real time passed.
#[test]
fn a_removed_outputs_global_is_destroyed_after_its_lifetime_on_yunas_clock() {
    let mut compositor = manual_compositor();
    let second = compositor
        .add_output(OWNER_SEAT, full_hd())
        .expect("an output is added");
    let mut client = Client::connect(&mut compositor);
    compositor
        .remove_output(second)
        .expect("the output is removed");
    client.roundtrip(&mut compositor);
    let [name] = client.state.removed_globals[..] else {
        panic!("one global removed: {:?}", client.state.removed_globals);
    };

    compositor
        .advance(RETIRED_GLOBAL_LIFETIME - Duration::from_millis(1))
        .expect("the manual clock moves");
    client.bind_output(name);
    client.roundtrip(&mut compositor);

    compositor
        .advance(Duration::from_millis(1))
        .expect("the manual clock moves");
    client.bind_output(name);
    assert!(
        client.try_roundtrip(&mut compositor).is_err(),
        "the global was destroyed: binding it is a protocol error"
    );
}

/// A compositor on the manual clock whose owner's seat has one output of `mode`.
fn compositor_with(mode: OutputMode) -> Compositor {
    Compositor::new(Config {
        outputs: vec![mode],
        clock: ClockMode::Manual,
        listen: false,
        ..Config::default()
    })
    .expect("the compositor starts")
}

/// Opens a window, acknowledges its first configure and commits a buffer of `colour` at the
/// size it asks; nothing is drawn yet.
fn open_at_asked_size(compositor: &mut Compositor, client: &mut Client, colour: u32) -> usize {
    let window = client.open_window(compositor);
    commit_asked_size(compositor, client, window, colour);
    window
}

/// Acknowledges `window`'s last configure and commits a buffer of `colour` at its size.
fn commit_asked_size(compositor: &mut Compositor, client: &mut Client, window: usize, colour: u32) {
    client.ack(window);
    let asked = client
        .window(window)
        .configures
        .last()
        .expect("a configure")
        .clone();
    client.attach(window, asked.width, asked.height, colour);
    client.commit(window);
    client.roundtrip(compositor);
}

/// Where `output` shows each of its windows, from its top-left corner, in logical units.
fn places_on(compositor: &Compositor, output: OutputId) -> Vec<(f64, f64, f64, f64)> {
    let corner = compositor
        .layout()
        .output_rect(output)
        .expect("the output exists");
    compositor
        .layout()
        .arrange(output)
        .expect("the output exists")
        .iter()
        .map(|p| (p.rect.x - corner.x, p.rect.y - corner.y, p.rect.w, p.rect.h))
        .collect()
}

/// An agent's output given a smaller mode, 1920 × 1080 to 1280 × 720, tells its clients the new
/// mode at once; at its next refresh its two windows are asked half its width less the gaps,
/// 616 × 688 where they were 936 × 1048, and placed again inside it, at those sizes before their
/// clients commit them. A capture after the change is 1280 × 720, and shows the windows where
/// they now are.
#[test]
fn an_output_made_smaller_places_its_windows_inside_it_and_is_captured_at_its_new_size() {
    let mut compositor = manual_compositor();
    let seat = compositor.add_seat().expect("an agent's seat");
    let output = compositor
        .add_output(seat, full_hd())
        .expect("an output is added");
    let mut client = Client::connect_to(&mut compositor, seat);
    let first = open_at_asked_size(&mut compositor, &mut client, RED);
    let second = open_at_asked_size(&mut compositor, &mut client, BLUE);
    run_armed_deadlines(&mut compositor);
    client.roundtrip(&mut compositor);
    assert_eq!(
        places_on(&compositor, output),
        vec![(16.0, 16.0, 936.0, 1048.0), (968.0, 16.0, 936.0, 1048.0)]
    );
    let global = client.output_named("HEADLESS-2").expect("its output");
    assert_eq!(
        client.state.output_modes,
        vec![(global, (1920, 1080, 60_000))]
    );

    let smaller = OutputMode::new(1280, 720, 60.0, 1.0).expect("a valid mode");
    compositor
        .configure_output(output, smaller)
        .expect("the output takes the mode");
    client.roundtrip(&mut compositor);
    assert_eq!(
        client.state.output_modes,
        vec![
            (global, (1920, 1080, 60_000)),
            (global, (1280, 720, 60_000))
        ]
    );
    let description = &compositor.output_descriptions()[1];
    assert_eq!((description.rect.w, description.rect.h), (1280.0, 720.0));

    run_armed_deadlines(&mut compositor);
    client.roundtrip(&mut compositor);
    // Placed for the sizes they are asked, before their clients answer.
    assert_eq!(
        places_on(&compositor, output),
        vec![(16.0, 16.0, 616.0, 688.0), (648.0, 16.0, 616.0, 688.0)]
    );
    for window in [first, second] {
        let asked = client
            .window(window)
            .configures
            .last()
            .expect("a configure");
        assert_eq!((asked.width, asked.height), (616, 688));
        commit_asked_size(&mut compositor, &mut client, window, RED);
    }
    run_armed_deadlines(&mut compositor);
    assert_eq!(
        places_on(&compositor, output),
        vec![(16.0, 16.0, 616.0, 688.0), (648.0, 16.0, 616.0, 688.0)]
    );
    assert_eq!(client.window(first).on_outputs, vec![global]);
    compositor
        .check_windows()
        .expect("the layout and the shell agree");

    let ticket = compositor
        .capture(
            seat,
            CaptureTarget::Output {
                output: Some(output),
                region: None,
            },
            CaptureSize::Physical,
        )
        .expect("a capture");
    run_armed_deadlines(&mut compositor);
    let captured = compositor.take_captures();
    let [(taken, Ok(frame))] = &captured[..] else {
        panic!("one capture, copied: {captured:?}");
    };
    assert_eq!(*taken, ticket);
    assert_eq!((frame.mapping.width, frame.mapping.height), (1280, 720));
    let image = frame.image();
    assert_eq!(image.len(), 1280 * 720 * 3);
    let pixel = |x: usize, y: usize| image[(y * 1280 + x) * 3..][..3].to_vec();
    // The second window, where it now is, and the background past it.
    assert_eq!(pixel(648 + 300, 300), vec![255, 0, 0]);
    assert_ne!(pixel(1270, 300), vec![255, 0, 0]);
}

/// A new rate starts the output's refresh boundaries again from the change, its refreshes
/// numbered on: at 60 Hz, boundary 1 presents the first frame, and boundaries 2 to 6 pass by
/// 105 ms; set to 120 Hz then, the next frames are presented 1/120 s and 2/120 s after the
/// change, refreshes 7 and 8, told the new period. Boundaries kept from the output's creation
/// would have presented them at 108.3 ms and 116.7 ms.
#[test]
fn a_new_rate_starts_the_refresh_boundaries_again_from_the_change() {
    let mut compositor = compositor_with(full_hd());
    let mut client = Client::connect(&mut compositor);
    let start = compositor.now();
    let window = client.open_window(&mut compositor);
    client.ack(window);
    client.attach(window, 936, 1048, RED);
    client.request_feedback(window);
    client.commit(window);
    client.roundtrip(&mut compositor);
    advance_to(&mut compositor, &mut client, start + PERIOD_AT_60);
    assert_eq!(
        client.window(window).feedback,
        vec![Feedback::Presented {
            time: start + PERIOD_AT_60,
            refresh: PERIOD_AT_60,
            seq: 1,
            flags: Kind::Vsync,
        }]
    );

    let change = start + Duration::from_millis(105);
    advance_to(&mut compositor, &mut client, change);
    compositor
        .configure_output(
            OUTPUT,
            OutputMode::new(1920, 1080, 120.0, 1.0).expect("a valid mode"),
        )
        .expect("the output takes the rate");
    let period = Duration::from_nanos(8_333_333);
    for (k, colour) in [(1u32, BLUE), (2, RED)] {
        client.attach(window, 936, 1048, colour);
        client.request_feedback(window);
        client.commit(window);
        client.roundtrip(&mut compositor);
        let boundary = change + Duration::from_nanos(u64::from(k) * 1_000_000_000 / 120);
        advance_to(&mut compositor, &mut client, boundary);
        assert_eq!(
            client.window(window).feedback.last(),
            Some(&Feedback::Presented {
                time: boundary,
                refresh: period,
                seq: 6 + u64::from(k),
                flags: Kind::Vsync,
            })
        );
    }
}

/// A mode the compositor cannot take is refused and changes nothing. Scale 0, a rate past
/// 1,000 Hz and a VRR range narrower than twice its least rate are refused as the mode is made;
/// an unknown output, and a side past the renderer's limit, by the change itself. The window
/// keeps its place, its client is told nothing, and nothing is armed.
#[test]
fn a_mode_refused_changes_nothing() {
    let mut compositor = manual_compositor();
    let mut client = Client::connect(&mut compositor);
    let window = open_and_commit(&mut compositor, &mut client, RED);
    run_armed_deadlines(&mut compositor);
    client.roundtrip(&mut compositor);
    let before = (
        compositor.output_descriptions(),
        compositor.windows(),
        compositor.layout().arrange(OUTPUT),
    );
    let told = (
        client.state.output_modes.clone(),
        client.state.output_scales.clone(),
        client.window(window).configures.len(),
    );

    assert!(OutputMode::new(3840, 2160, 240.0, 0.0).is_err());
    assert!(OutputMode::new(3840, 2160, 1_001.0, 1.5).is_err());
    let mode = OutputMode::new(3840, 2160, 240.0, 1.5).expect("a valid mode");
    assert!(mode.with_vrr(200.0).is_err());
    assert!(matches!(
        compositor.configure_output(OutputId(9), mode),
        Err(Error::Refused(_))
    ));
    let too_wide = OutputMode::new(1_000_000, 2160, 240.0, 1.5).expect("a mode to check");
    assert!(matches!(
        compositor.configure_output(OUTPUT, too_wide),
        Err(Error::InvalidOutput(_))
    ));
    // Its own mode again is no change.
    compositor
        .configure_output(OUTPUT, OutputMode::default())
        .expect("its own mode");

    client.roundtrip(&mut compositor);
    assert_eq!(compositor.next_deadline(), None);
    assert_eq!(
        (
            compositor.output_descriptions(),
            compositor.windows(),
            compositor.layout().arrange(OUTPUT),
        ),
        before
    );
    assert_eq!(
        (
            client.state.output_modes.clone(),
            client.state.output_scales.clone(),
            client.window(window).configures.len(),
        ),
        told
    );
}

/// A fullscreen game that draws 1280 × 720 whatever it is asked is centred on 1920 × 1080, at
/// (320, 180). The output given 2560 × 1440, the game is asked the whole output and placed there
/// until it answers: a frame it commits without acknowledging the new configure does not count,
/// and one that never answers stays so. Once it acknowledges the configure and commits
/// 1280 × 720 again, that commit reaches the layout though its size did not change: the game is
/// centred again, at (640, 360).
#[test]
fn a_game_that_keeps_its_size_is_centred_again_once_it_answers_the_new_mode() {
    let mut compositor = compositor_with(full_hd());
    let mut client = Client::connect(&mut compositor);
    let game = client.open_window(&mut compositor);
    client.set_fullscreen(game, true);
    client.roundtrip(&mut compositor);
    client.ack(game);
    client.attach(game, 1280, 720, RED);
    client.commit(game);
    client.roundtrip(&mut compositor);
    run_armed_deadlines(&mut compositor);
    client.roundtrip(&mut compositor);
    assert_eq!(
        places_on(&compositor, OUTPUT),
        vec![(320.0, 180.0, 1280.0, 720.0)]
    );

    compositor
        .configure_output(
            OUTPUT,
            OutputMode::new(2560, 1440, 60.0, 1.0).expect("a valid mode"),
        )
        .expect("the output takes the mode");
    run_armed_deadlines(&mut compositor);
    client.roundtrip(&mut compositor);
    let asked = client
        .window(game)
        .configures
        .last()
        .expect("a configure")
        .clone();
    assert_eq!((asked.width, asked.height), (2560, 1440));
    assert!(asked.states.contains(&crate::client::State::Fullscreen));
    // A frame of the game's own, the new configure read but not acknowledged.
    client.attach(game, 1280, 720, BLUE);
    client.commit(game);
    client.roundtrip(&mut compositor);
    run_armed_deadlines(&mut compositor);
    compositor
        .advance(Duration::from_secs(5))
        .expect("the manual clock moves");
    assert_eq!(
        places_on(&compositor, OUTPUT),
        vec![(0.0, 0.0, 2560.0, 1440.0)]
    );

    client.ack(game);
    client.attach(game, 1280, 720, RED);
    client.commit(game);
    client.roundtrip(&mut compositor);
    run_armed_deadlines(&mut compositor);
    assert_eq!(
        places_on(&compositor, OUTPUT),
        vec![(640.0, 360.0, 1280.0, 720.0)]
    );
    assert!(!compositor.layout().awaits_commit(WindowId(1)));
}

/// Runs every deadline armed, and those they arm, until none is: at most 200, so that one armed
/// again and again fails the story.
fn run_until_rest(compositor: &mut Compositor) {
    for _ in 0..200 {
        let Some(at) = compositor.next_deadline() else {
            return;
        };
        compositor
            .advance(at.saturating_sub(compositor.now()))
            .expect("the manual clock moves");
    }
    panic!("deadlines keep coming");
}

/// A mode the layout refuses after the compositor's own checks passed and the new buffer was
/// made changes nothing: the output keeps its mode and its buffer, its window its place, its
/// client hears nothing, and nothing is armed. Outputs 16,384 × 1 pixels at 0.25, 65,536 units
/// wide each, put one past 985,600 units; output 1 given that mode, 62,976 units wider, would
/// move it past the farthest place an output may lie, 1,000,000.
#[test]
fn a_mode_the_layout_refuses_after_the_buffer_was_made_changes_nothing() {
    let mut compositor = manual_compositor();
    let mut client = Client::connect(&mut compositor);
    let window = open_and_commit(&mut compositor, &mut client, RED);
    let wide = OutputMode::new(16_384, 1, 60.0, 0.25).expect("a valid mode");
    let added: Vec<OutputId> = (0..MAX_OUTPUTS - 1)
        .map(|_| {
            compositor
                .add_output(OWNER_SEAT, wide)
                .expect("an output within the renderer's limit")
        })
        .collect();
    let (last, middle) = added.split_last().expect("outputs added");
    for &output in middle {
        compositor
            .remove_output(output)
            .expect("the output is removed");
    }
    let farthest = compositor
        .add_output(OWNER_SEAT, wide)
        .expect("an output at the right edge");
    run_until_rest(&mut compositor);
    client.roundtrip(&mut compositor);
    let places = |compositor: &Compositor| {
        compositor
            .output_descriptions()
            .iter()
            .map(|output| (output.id, output.rect.x))
            .collect::<Vec<_>>()
    };
    assert_eq!(
        places(&compositor),
        vec![(OUTPUT, 0.0), (*last, 920_064.0), (farthest, 985_600.0)]
    );
    let global = client.output_named("HEADLESS-1").expect("output 1");
    let told = |client: &Client| {
        let modes: Vec<_> = client
            .state
            .output_modes
            .iter()
            .filter(|(name, _)| *name == global)
            .copied()
            .collect();
        let scales: Vec<_> = client
            .state
            .output_scales
            .iter()
            .filter(|(name, _)| *name == global)
            .copied()
            .collect();
        (modes, scales)
    };
    let before = (
        compositor.output_descriptions(),
        compositor.windows(),
        compositor.layout().arrange(OUTPUT),
        told(&client),
        client.window(window).configures.len(),
    );

    let refused = compositor.configure_output(OUTPUT, wide);
    assert!(
        matches!(&refused, Err(Error::InvalidOutput(reason)) if reason.0.contains("layout")),
        "{refused:?}"
    );
    client.roundtrip(&mut compositor);
    assert_eq!(compositor.next_deadline(), None);
    assert_eq!(
        (
            compositor.output_descriptions(),
            compositor.windows(),
            compositor.layout().arrange(OUTPUT),
            told(&client),
            client.window(window).configures.len(),
        ),
        before
    );
    // The buffer is still 3840 × 2160: its last pixel reads, which a 16,384 × 1 one lacks.
    let corner = smithay::utils::Rectangle::new((3839, 2159).into(), (1, 1).into());
    assert_eq!(
        compositor
            .read_pixels(OUTPUT, corner)
            .expect("the last pixel reads")
            .len(),
        4
    );
}
