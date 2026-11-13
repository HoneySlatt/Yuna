//! The owner's peek at a workspace not shown: the model arranges any workspace of an output as
//! that output would show it, and changes nothing ([ADR-0015](../../../../docs/adr/0015-agents-workspaces.md),
//! [ADR-0020](../../../../docs/adr/0020-the-layout-models-shape.md)).

use crate::harness::*;
use yuna_layout::{LayoutError, OpenOptions, Placed};

fn floating() -> OpenOptions {
    OpenOptions {
        floating: true,
        ..OpenOptions::default()
    }
}

/// Peeks at `output`'s workspace `index`, and checks that the model did not change.
#[track_caller]
fn peek(m: &Model, output: yuna_layout::OutputId, index: usize) -> Vec<Placed> {
    let before = format!("{:?}", m.layout);
    let placed = m.layout.arrange_workspace(output, index).unwrap();
    assert_eq!(
        before,
        format!("{:?}", m.layout),
        "a peek changed the model"
    );
    placed
}

/// A workspace switched away from is arranged as it was shown, at scale 1.5 too, its rectangles
/// on the output's physical grid; only the focus differs, which stays with the workspace shown.
#[test]
fn a_workspace_not_shown_is_arranged_as_its_output_showed_it() {
    let mut m = Model::bare(Default::default());
    m.ok(|l| l.add_seat(AGENT));
    m.ok(|l| l.add_output(AGENT, MAIN, output(0.0, 3840, 2160, 1.5)));
    m.ok(|l| l.add_workspace(AGENT, "code", None));
    m.ok(|l| l.add_workspace(AGENT, "web", None));
    m.ok(|l| l.focus_workspace(AGENT, "code"));
    m.open_with(AGENT, 1, OpenOptions::default());
    m.open_with(AGENT, 2, OpenOptions::default());
    m.open_with(AGENT, 3, floating());
    let shown = m.placed(MAIN);
    let code = m
        .layout
        .workspaces(MAIN)
        .unwrap()
        .iter()
        .position(|w| w.name.as_deref() == Some("code"))
        .unwrap();

    m.ok(|l| l.focus_workspace(AGENT, "web"));
    m.open_with(AGENT, 4, OpenOptions::default());
    let peeked = peek(&m, MAIN, code);
    assert_eq!(peeked.len(), 3);
    for (seen, was) in peeked.iter().zip(&shown) {
        assert_eq!(seen.window, was.window);
        assert_eq!(seen.rect, was.rect);
        assert_eq!(seen.configure, was.configure);
        assert_eq!(seen.tiled, was.tiled);
        assert!(!seen.activated, "the seat's focus is on window 4");
    }
    for p in &peeked {
        for edge in [p.rect.x, p.rect.y, p.rect.w, p.rect.h] {
            let pixels = edge * 1.5;
            assert!(
                (pixels - pixels.round()).abs() < 1e-9,
                "{p:?} is off the grid"
            );
        }
    }
    assert_eq!(m.shown(MAIN), vec![4], "the output still shows web alone");
}

/// The workspace shown peeks as the output draws it, its pinned window last, without the special
/// workspace shown over it.
#[test]
fn the_workspace_shown_peeks_without_the_special_workspace_over_it() {
    let mut m = Model::new();
    m.open(1);
    m.open_with(OWNER, 2, floating());
    m.ok(|l| l.set_pinned(w(2), true));
    m.open(3);
    m.ok(|l| l.toggle_special(OWNER, "scratch"));
    m.open(4);
    assert_eq!(m.shown(MAIN), vec![1, 3, 4, 2]);
    let shown = m.workspaces(MAIN).1;
    let peeked = peek(&m, MAIN, shown);
    assert_eq!(
        peeked.iter().map(|p| p.window.0).collect::<Vec<_>>(),
        vec![1, 3, 2]
    );
    let drawn = m.placed(MAIN);
    for p in &peeked {
        assert!(
            drawn.contains(p),
            "{p:?} differs from what the output draws"
        );
    }
}

/// A fullscreen window that held its workspace's focus hides its floating windows in the peek,
/// as it did on its output.
#[test]
fn a_fullscreen_window_covers_its_hidden_workspace_in_the_peek() {
    let mut m = Model::new();
    m.ok(|l| l.add_workspace(OWNER, "game", None));
    m.ok(|l| l.add_workspace(OWNER, "desk", None));
    m.ok(|l| l.focus_workspace(OWNER, "game"));
    m.open_with(OWNER, 1, floating());
    m.open(2);
    m.ok(|l| l.set_fullscreen(w(2), true));
    let game = m.workspaces(MAIN).1;
    m.ok(|l| l.focus_workspace(OWNER, "desk"));
    assert_ne!(m.workspaces(MAIN).1, game);
    let peeked = peek(&m, MAIN, game);
    assert_eq!(peeked.len(), 1);
    assert_eq!(peeked[0].window, w(2));
    assert!(peeked[0].fullscreen);
    assert_rect(peeked[0].rect, rect(0.0, 0.0, 1920.0, 1080.0));
}

/// The owner peeks at an agent's workspace: a peek reads another seat's branch and changes it
/// not. An unknown output and an index past the workspaces are refused.
#[test]
fn the_owner_peeks_at_an_agents_workspace_and_bad_requests_are_refused() {
    let mut m = Model::new();
    m.ok(|l| l.add_seat(AGENT));
    m.ok(|l| l.add_output(AGENT, SIDE, output(1920.0, 1920, 1080, 1.0)));
    m.open_with(AGENT, 1, OpenOptions::default());
    let peeked = peek(&m, SIDE, 0);
    assert_eq!(peeked.len(), 1);
    assert_eq!(peeked[0].window, w(1));
    assert!(peeked[0].activated, "it holds the agent's focus");
    assert_eq!(peeked[0].rect, m.rect(1));

    let count = m.layout.workspaces(SIDE).unwrap().len();
    assert_eq!(
        m.layout.arrange_workspace(SIDE, count),
        Err(LayoutError::InvalidValue)
    );
    let unknown = yuna_layout::OutputId(9);
    assert_eq!(
        m.layout.arrange_workspace(unknown, 0),
        Err(LayoutError::UnknownOutput(unknown))
    );
}

/// A special workspace's fullscreen window hides the pinned windows on the output, but not in
/// the peek at the workspace beneath, which shows that workspace alone. Found by the property
/// tests in M1.5's closing review.
#[test]
fn a_pinned_window_hidden_by_a_covering_special_workspace_stays_in_the_peek() {
    let mut m = Model::new();
    m.open(1);
    m.open_with(OWNER, 2, floating());
    m.ok(|l| l.set_pinned(w(2), true));
    m.ok(|l| l.toggle_special(OWNER, "scratch"));
    m.open(4);
    m.ok(|l| l.set_fullscreen(w(4), true));
    assert_eq!(m.shown(MAIN), vec![1, 4]);
    let shown = m.workspaces(MAIN).1;
    let peeked: Vec<u64> = peek(&m, MAIN, shown).iter().map(|p| p.window.0).collect();
    assert_eq!(peeked, vec![1, 2]);
}
