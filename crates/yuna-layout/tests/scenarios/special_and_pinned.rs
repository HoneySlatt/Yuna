//! Special workspaces and the pinned window, Hyprland's, read in its wiki at commit f20353c
//! (configuring/core/dispatchers.md) and its source at commit 70ec30e.

use crate::harness::*;
use yuna_layout::{LayoutError, OpenOptions, Options};

fn floating() -> OpenOptions {
    OpenOptions {
        floating: true,
        ..OpenOptions::default()
    }
}

/// Hyprland: `togglespecialworkspace` shows it over the current workspace, focus inside; new
/// windows open in it; hiding gives the focus back.
#[test]
fn a_special_workspace_shows_over_the_current_one_and_takes_the_focus_while_shown() {
    let mut m = Model::new();
    m.open(1);
    m.ok(|l| l.toggle_special(OWNER, "scratch"));
    assert_eq!(
        m.layout.shown_special(MAIN).unwrap().as_deref(),
        Some("scratch")
    );
    assert_eq!(m.focused(), None);
    m.open(2);
    assert!(m.info(2).special);
    assert_eq!(m.focused(), Some(2));
    assert_eq!(
        m.shown(MAIN),
        vec![1, 2],
        "the special workspace is drawn over"
    );
    m.ok(|l| l.toggle_special(OWNER, "scratch"));
    assert_eq!(m.shown(MAIN), vec![1]);
    assert_eq!(m.focused(), Some(1));
    assert_eq!(m.info(2).output, None);
    m.ok(|l| l.toggle_special(OWNER, "scratch"));
    assert_eq!(m.focused(), Some(2));
}

/// Hyprland: a special workspace shown on another monitor moves to the focused one.
#[test]
fn a_special_workspace_comes_to_the_active_output() {
    let mut m = Model::new();
    m.ok(|l| l.add_output(OWNER, SIDE, output(1920.0, 1920, 1080, 1.0)));
    m.ok(|l| l.toggle_special(OWNER, "scratch"));
    m.open(1);
    m.ok(|l| l.focus_output(OWNER, SIDE));
    m.ok(|l| l.toggle_special(OWNER, "scratch"));
    assert_eq!(m.layout.shown_special(MAIN).unwrap(), None);
    assert_eq!(
        m.layout.shown_special(SIDE).unwrap().as_deref(),
        Some("scratch")
    );
    assert_eq!(m.info(1).output, Some(SIDE));
}

/// Hyprland: `movetoworkspacesilent special:…` moves the window without showing it.
#[test]
fn a_window_put_aside_in_a_special_workspace_hides_until_it_is_shown() {
    let mut m = Model::new();
    m.open(1);
    m.open(2);
    m.ok(|l| l.move_to_special(OWNER, "scratch"));
    assert_eq!(m.shown(MAIN), vec![1]);
    assert_eq!(m.focused(), Some(1));
    m.ok(|l| l.focus_window(w(2)));
    assert_eq!(
        m.layout.shown_special(MAIN).unwrap().as_deref(),
        Some("scratch")
    );
    assert_eq!(m.focused(), Some(2));
}

/// Hyprland: `binds:hide_special_on_workspace_change` false (src/config ConfigValues.cpp).
#[test]
fn a_special_workspace_stays_shown_when_the_workspace_beneath_changes() {
    let mut m = Model::new();
    m.ok(|l| l.add_workspace(OWNER, "web", None));
    m.ok(|l| l.toggle_special(OWNER, "scratch"));
    m.open(1);
    m.ok(|l| l.focus_workspace(OWNER, "web"));
    assert_eq!(
        m.layout.shown_special(MAIN).unwrap().as_deref(),
        Some("scratch")
    );
    assert_eq!(m.focused(), Some(1));
}

/// Hyprland: `misc:close_special_on_empty` true.
#[test]
fn a_special_workspace_closes_when_its_last_window_leaves() {
    let mut m = Model::new();
    m.open(1);
    m.ok(|l| l.toggle_special(OWNER, "scratch"));
    m.open(2);
    m.ok(|l| l.close_window(w(2)));
    assert_eq!(m.layout.shown_special(MAIN).unwrap(), None);
    assert_eq!(m.focused(), Some(1));
    // Toggled on while empty, it stays until toggled off.
    m.ok(|l| l.toggle_special(OWNER, "scratch"));
    assert_eq!(
        m.layout.shown_special(MAIN).unwrap().as_deref(),
        Some("scratch")
    );
}

/// Yuna's choice (ADR-0019): focusing a window beneath a special workspace hides it, so that the
/// window has the focus.
#[test]
fn focusing_a_window_beneath_a_special_workspace_hides_it() {
    let mut m = Model::new();
    m.open(1);
    m.ok(|l| l.toggle_special(OWNER, "scratch"));
    m.open(2);
    m.ok(|l| l.focus_window(w(1)));
    assert_eq!(m.layout.shown_special(MAIN).unwrap(), None);
    assert_eq!(m.focused(), Some(1));
}

/// Hyprland: src/render/Renderer.cpp, "pinned always above".
#[test]
fn a_pinned_window_is_drawn_over_the_special_workspace() {
    let mut m = Model::new();
    m.open_with(OWNER, 1, floating());
    m.ok(|l| l.set_pinned(w(1), true));
    m.ok(|l| l.toggle_special(OWNER, "scratch"));
    m.open(2);
    assert_eq!(m.shown(MAIN), vec![2, 1]);
}

/// Hyprland: a pinned window follows workspace switches on its monitor.
#[test]
fn a_pinned_window_follows_the_workspaces_its_output_shows_without_taking_focus() {
    let mut m = Model::new();
    m.ok(|l| l.add_workspace(OWNER, "web", None));
    m.open_with(OWNER, 1, floating());
    m.ok(|l| l.set_floating_rect(w(1), rect(50.0, 60.0, 320.0, 240.0)));
    m.ok(|l| l.set_pinned(w(1), true));
    m.ok(|l| l.focus_workspace(OWNER, "web"));
    m.open(2);
    assert_eq!(m.info(1).workspace.as_deref(), Some("web"));
    assert_rect(m.rect(1), rect(50.0, 60.0, 320.0, 240.0));
    assert_eq!(m.focused(), Some(2));
    // Unpinned, it stays where it is.
    m.ok(|l| l.set_pinned(w(1), false));
    m.ok(|l| l.focus_workspace_relative(OWNER, yuna_layout::Direction::Up));
    assert_eq!(m.info(1).workspace.as_deref(), Some("web"));
}

/// Hyprland: `pin` applies to floating windows only; a pinned window cannot be fullscreen
/// (`binds:allow_pin_fullscreen` false).
#[test]
fn only_a_floating_window_is_pinned_and_a_pinned_one_is_not_fullscreen() {
    let mut m = Model::new();
    m.open(1);
    m.refused(
        LayoutError::Refused("only a floating window can be pinned"),
        |l| l.set_pinned(w(1), true),
    );
    m.open_with(OWNER, 2, floating());
    m.ok(|l| l.set_pinned(w(2), true));
    m.refused(
        LayoutError::Refused("a pinned window cannot be fullscreen"),
        |l| l.set_fullscreen(w(2), true),
    );
    m.ok(|l| l.toggle_special(OWNER, "scratch"));
    m.open_with(OWNER, 3, floating());
    m.refused(
        LayoutError::Refused("only a window of a shown workspace can be pinned"),
        |l| l.set_pinned(w(3), true),
    );
}

/// niri's rule over Hyprland's: while a fullscreen window holds the focus, floating and pinned
/// windows hide (ADR-0019).
#[test]
fn floating_and_pinned_windows_hide_while_a_fullscreen_window_holds_the_focus() {
    let mut m = Model::with(Options::default());
    m.open(1);
    m.open_with(OWNER, 2, floating());
    m.ok(|l| l.set_pinned(w(2), true));
    m.ok(|l| l.set_fullscreen(w(1), true));
    assert_eq!(m.shown(MAIN), vec![1]);
    m.ok(|l| l.focus_window(w(2)));
    assert_eq!(m.shown(MAIN), vec![1, 2]);
}
