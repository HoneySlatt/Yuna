//! The words of a refusal: a caller shows them to whoever asked, so each says what was refused.

use crate::harness::*;
use yuna_layout::{LayoutError, OutputId, SeatId};

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
            LayoutError::Refused("the window is tiled"),
            "the window is tiled",
        ),
    ];
    for (error, text) in messages {
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
