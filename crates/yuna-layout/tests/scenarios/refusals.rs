//! The reasons of a refusal: the model refuses each operation with the reason that says what was
//! refused. Its sentence is written in `yuna_ipc::Refusal` alone, which the compositor turns each
//! reason into (`crates/yuna/src/command.rs`), so that Yuna's text lives in one module
//! ([ADR-0082](../../../../docs/adr/0082-the-requirements-of-stage-2s-core-its-bets-made-rules-or-deferred.md)
//! item 23).

use crate::harness::*;
use yuna_layout::LayoutError;

/// The refusals the model itself gives name their reason, and what they refused.
#[test]
fn the_model_refuses_with_its_reasons() {
    let mut m = Model::new();
    let error = m.layout.close_window(w(1)).unwrap_err();
    assert_eq!(error, LayoutError::UnknownWindow(w(1)));
    let error = m
        .layout
        .add_output(OWNER, MAIN, output(0.0, 1920, 1080, 1.0));
    assert_eq!(error.unwrap_err(), LayoutError::AlreadyExists);
    m.open(1);
    let error = m.layout.set_floating_rect(w(1), rect(0.0, 0.0, 10.0, 10.0));
    assert_eq!(error.unwrap_err(), LayoutError::WindowTiled);
}
