//! `xdg-dialog-v1`: a toplevel's modal hint is recorded and taken back; its dialog object or its
//! toplevel may go first.

use yuna::yuna_layout::WindowId;

use crate::client::Client;
use crate::{BLUE, RED, advance_to, assert_rests, boundary, manual_compositor, open_and_commit};

/// Story 8: a modal dialog is recorded, then unset. Its dialog object destroyed first takes the
/// hint back; its toplevel destroyed first, then the dialog object, is survived, and the other
/// window keeps drawing.
#[test]
fn a_modal_hint_is_recorded_and_either_object_may_go_first() {
    let mut compositor = manual_compositor();
    let mut client = Client::connect(&mut compositor);
    let start = compositor.now();
    let parent = open_and_commit(&mut compositor, &mut client, RED);
    let child = open_and_commit(&mut compositor, &mut client, BLUE);
    client.set_parent(child, parent);
    let dialog = client.get_dialog(child);
    client.roundtrip(&mut compositor);
    advance_to(&mut compositor, &mut client, boundary(start, 1));
    assert!(!compositor.is_modal(WindowId(2)));

    client.set_modal(dialog, true);
    client.roundtrip(&mut compositor);
    assert!(compositor.is_modal(WindowId(2)));
    assert!(!compositor.is_modal(WindowId(1)));
    client.set_modal(dialog, false);
    client.roundtrip(&mut compositor);
    assert!(!compositor.is_modal(WindowId(2)));

    // The dialog object goes first: the hint goes with it.
    client.set_modal(dialog, true);
    client.destroy_dialog(dialog);
    client.roundtrip(&mut compositor);
    assert!(!compositor.is_modal(WindowId(2)));

    // The toplevel goes first, then its dialog object.
    let dialog = client.get_dialog(child);
    client.set_modal(dialog, true);
    client.roundtrip(&mut compositor);
    assert!(compositor.is_modal(WindowId(2)));
    client.close(child);
    client.roundtrip(&mut compositor);
    assert!(!compositor.is_modal(WindowId(2)));
    client.destroy_dialog(dialog);
    client.roundtrip(&mut compositor);

    client.request_frame(parent);
    client.commit(parent);
    client.roundtrip(&mut compositor);
    advance_to(&mut compositor, &mut client, boundary(start, 2));
    assert_eq!(client.window(parent).frames.len(), 2);
    assert_rests(&mut compositor);
}
