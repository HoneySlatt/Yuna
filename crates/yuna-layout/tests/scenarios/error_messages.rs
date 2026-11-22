//! The words of a refusal: a caller shows them to whoever asked, so each says what was refused.

use crate::harness::*;
use yuna_layout::LayoutError;
use yuna_types::{OutputId, SeatId};

#[test]
fn every_refusal_says_what_it_refuses() {
    let messages = [
        (LayoutError::UnknownSeat(SeatId(7)), "no seat 7"),
        (LayoutError::UnknownOutput(OutputId(8)), "no output 8"),
        (LayoutError::UnknownWindow(w(9)), "no window 9"),
        (
            LayoutError::UnknownWorkspace("code".to_owned()),
            "no workspace named \"code\"",
        ),
        (LayoutError::AlreadyExists, "this id is already used"),
        (
            LayoutError::InvalidValue,
            "a value is not finite or out of range",
        ),
        (LayoutError::NotEmpty, "it still holds windows"),
        (
            LayoutError::OutputOfAnotherSeat,
            "the output belongs to another seat",
        ),
        (LayoutError::SeatWithoutOutput, "the seat has no output"),
        (
            LayoutError::WorkspaceWithoutOutput,
            "the window's workspace has no output",
        ),
        (LayoutError::WindowTiled, "the window is tiled"),
        (
            LayoutError::PinnedNotFullscreen,
            "a pinned window cannot be fullscreen",
        ),
        (
            LayoutError::ScreenHeld,
            "another window's fullscreen holds the screen",
        ),
        (
            LayoutError::PinnedOutsideShownWorkspace,
            "only a window of a shown workspace can be pinned",
        ),
        (
            LayoutError::PinnedNotFloating,
            "only a floating window can be pinned",
        ),
    ];
    for (error, text) in messages {
        // Names every variant: one added fails to compile here, which points to the list above,
        // where its words go.
        match error {
            LayoutError::UnknownSeat(_)
            | LayoutError::UnknownOutput(_)
            | LayoutError::UnknownWindow(_)
            | LayoutError::UnknownWorkspace(_)
            | LayoutError::AlreadyExists
            | LayoutError::InvalidValue
            | LayoutError::NotEmpty
            | LayoutError::OutputOfAnotherSeat
            | LayoutError::SeatWithoutOutput
            | LayoutError::WorkspaceWithoutOutput
            | LayoutError::WindowTiled
            | LayoutError::PinnedNotFullscreen
            | LayoutError::ScreenHeld
            | LayoutError::PinnedOutsideShownWorkspace
            | LayoutError::PinnedNotFloating => {}
        }
        assert_eq!(error.to_string(), text);
    }
}

/// The refusals the model itself gives carry these words to the caller.
#[test]
fn the_model_refuses_with_its_words() {
    let mut m = Model::new();
    let error = m.layout.close_window(w(1)).unwrap_err();
    assert_eq!(error.to_string(), "no window 1");
    let error = m
        .layout
        .add_output(OWNER, MAIN, output(0.0, 1920, 1080, 1.0));
    assert_eq!(error.unwrap_err().to_string(), "this id is already used");
    m.open(1);
    let error = m.layout.set_floating_rect(w(1), rect(0.0, 0.0, 10.0, 10.0));
    assert_eq!(error.unwrap_err().to_string(), "the window is tiled");
}
