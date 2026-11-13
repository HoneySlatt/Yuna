//! Outputs created and removed while clients run, as the MCP server's `outputs` tool does
//! (M1.5), and what the compositor says of its windows (`Compositor::windows`).

use std::time::Duration;

use yuna::yuna_layout::{OutputId, WindowId};
use yuna::{Error, MAX_OUTPUTS, OWNER_SEAT, OutputMode};

use crate::client::{Client, Feedback};
use crate::{BLUE, LEAD, OUTPUT, RED, manual_compositor, open_and_commit, run_armed_deadlines};

/// 1 s / 60, to the nanosecond below.
const PERIOD_AT_60: Duration = Duration::from_nanos(16_666_666);

fn full_hd() -> OutputMode {
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
        .add_output(full_hd())
        .expect("an output is added");
    assert_eq!(second_output, OutputId(2));
    // Once for the global, once for the events of the output bound.
    client.roundtrip(&mut compositor);
    client.roundtrip(&mut compositor);
    let two = client
        .output_named("HEADLESS-2")
        .expect("the new output is announced");
    compositor
        .change_layout(|layout| layout.focus_output(OWNER_SEAT, second_output))
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
            .add_output(small)
            .expect("an output under the cap");
    }
    assert!(matches!(
        compositor.add_output(small),
        Err(Error::Refused(_))
    ));
    assert_eq!(compositor.outputs().len(), MAX_OUTPUTS);
    compositor
        .remove_output(OutputId(2))
        .expect("an output removed");
    assert_eq!(
        compositor.add_output(small).expect("room again"),
        OutputId(MAX_OUTPUTS as u64 + 1)
    );
}
