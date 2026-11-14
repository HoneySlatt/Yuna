//! The ids the model gives the workspaces and columns it creates, each window's layer, and each
//! shown workspace's camera: what following windows, columns and workspaces from one arrangement
//! to the next needs ([ADR-0052](../../../../docs/adr/0052-motion-in-a-pure-crate.md)). The
//! output is 1920 × 1080 at scale 1 with gaps of 16, so a column of half the width is 936 wide.

use crate::harness::*;
use yuna_layout::{
    ColumnId, ColumnWidth, Direction, Layer, LayoutError, OpenOptions, OutputId, Tiling,
    WorkspaceId,
};

/// A strip position, from one column's left edge to the next: a half column and a gap.
const STEP: f64 = 936.0 + 16.0;

fn floating() -> OpenOptions {
    OpenOptions {
        floating: true,
        ..OpenOptions::default()
    }
}

/// The column of window `n`.
#[track_caller]
fn column(m: &Model, n: u64) -> ColumnId {
    m.placement(n)
        .column
        .unwrap_or_else(|| panic!("window {n} is in no column"))
}

/// The id of the workspace `output` shows.
fn shown_id(m: &Model, output: OutputId) -> WorkspaceId {
    m.layout.shown_workspaces(output).unwrap().main.id
}

/// The id of the workspace named `name` among `output`'s.
#[track_caller]
fn named(m: &Model, output: OutputId, name: &str) -> WorkspaceId {
    m.layout
        .workspaces(output)
        .unwrap()
        .into_iter()
        .find(|w| w.name.as_deref() == Some(name))
        .unwrap_or_else(|| panic!("no workspace {name} on {output:?}"))
        .id
}

/// The camera of the workspace `output` shows.
#[track_caller]
fn view_x(m: &Model, output: OutputId) -> f64 {
    m.layout
        .shown_workspaces(output)
        .unwrap()
        .main
        .view_x
        .expect("scrolling columns")
}

/// Window `n`'s strip position, by [`yuna_layout::WorkspaceView`]'s rule:
/// `rect.x - working.x + view_x`.
#[track_caller]
fn strip_x(m: &Model, n: u64) -> f64 {
    let placed = m.placement(n);
    let shown = m.layout.shown_workspaces(MAIN).unwrap();
    let view = [Some(shown.main), shown.special]
        .into_iter()
        .flatten()
        .find(|v| v.id == placed.workspace)
        .expect("a window MAIN draws is on a workspace it shows");
    placed.rect.x - view.working.x + view.view_x.expect("scrolling columns")
}

/// Asserts that `id` is new, then remembers it.
#[track_caller]
fn assert_new<T: PartialEq + std::fmt::Debug + Copy>(seen: &mut Vec<T>, id: T) {
    assert!(!seen.contains(&id), "{id:?} was given before");
    seen.push(id);
}

/// Every way a workspace comes to be gives it a new id, the first 1, the owner's and an agent's
/// alike, even for one name on two seats.
#[test]
fn every_workspace_created_gets_a_new_id_whatever_its_seat() {
    let mut m = Model::with(yuna_layout::Options {
        dynamic_workspaces: true,
        ..Default::default()
    });
    // The output without a workspace got an unnamed one; dynamic workspaces keep it last.
    assert_eq!(m.workspace_id(MAIN, 0), WorkspaceId(1));
    let mut seen = vec![WorkspaceId(1)];
    m.ok(|l| l.add_workspace(OWNER, "1", None));
    assert_new(&mut seen, named(&m, MAIN, "1"));
    m.ok(|l| l.focus_workspace(OWNER, "web"));
    assert_new(&mut seen, named(&m, MAIN, "web"));
    m.open_with(
        OWNER,
        1,
        OpenOptions {
            workspace: Some("mail".into()),
            ..OpenOptions::default()
        },
    );
    assert_new(&mut seen, named(&m, MAIN, "mail"));
    m.ok(|l| l.focus_window(w(1)));
    m.ok(|l| l.move_to_workspace(OWNER, "chat"));
    assert_new(&mut seen, named(&m, MAIN, "chat"));
    // "chat" came last: dynamic workspaces put a new empty one after it.
    let last = m.layout.workspaces(MAIN).unwrap().len() - 1;
    assert_new(&mut seen, m.workspace_id(MAIN, last));
    m.ok(|l| l.toggle_special(OWNER, "scratch"));
    let special = m.layout.shown_workspaces(MAIN).unwrap().special.unwrap();
    assert_new(&mut seen, special.id);
    m.ok(|l| l.toggle_special(OWNER, "scratch"));
    m.ok(|l| l.move_to_special(OWNER, "aside"));
    m.ok(|l| l.toggle_special(OWNER, "aside"));
    let aside = m.layout.shown_workspaces(MAIN).unwrap().special.unwrap();
    assert_new(&mut seen, aside.id);

    m.ok(|l| l.add_seat(AGENT));
    m.ok(|l| l.add_output(AGENT, SIDE, output(1920.0, 1920, 1080, 1.0)));
    assert_new(&mut seen, m.workspace_id(SIDE, 0));
    m.ok(|l| l.add_workspace(AGENT, "1", None));
    assert_new(&mut seen, named(&m, SIDE, "1"));
}

/// The first column gets id 1; each new one a new id, on any workspace of any seat.
#[test]
fn every_column_created_gets_a_new_id_whatever_its_workspace_or_seat() {
    let mut m = Model::new();
    m.open(1);
    assert_eq!(column(&m, 1), ColumnId(1));
    let mut seen = vec![ColumnId(1)];
    m.open(2);
    assert_new(&mut seen, column(&m, 2));
    m.ok(|l| l.focus_workspace(OWNER, "other"));
    m.open(3);
    assert_new(&mut seen, column(&m, 3));
    m.ok(|l| l.add_seat(AGENT));
    m.ok(|l| l.add_output(AGENT, SIDE, output(1920.0, 1920, 1080, 1.0)));
    m.open_with(AGENT, 4, OpenOptions::default());
    assert_new(&mut seen, column(&m, 4));
}

/// A workspace whose output goes moves to another and comes back with its id; so does one that
/// waits without an output while its seat has none.
#[test]
fn a_workspace_keeps_its_id_when_its_output_goes_and_comes_back() {
    let mut m = Model::new();
    m.ok(|l| l.add_workspace(OWNER, "2", Some(SIDE)));
    m.ok(|l| l.add_output(OWNER, SIDE, output(1920.0, 1920, 1080, 1.0)));
    m.ok(|l| l.focus_output(OWNER, SIDE));
    m.open(1);
    let two = named(&m, SIDE, "2");
    // An unnamed workspace with a window, at home on SIDE too.
    m.ok(|l| l.move_to_workspace(OWNER, "3"));
    m.ok(|l| l.remove_workspace(OWNER, "3"));
    assert_eq!(
        m.layout.window(w(1)).unwrap().workspace,
        None,
        "3 lost its name"
    );
    let unnamed = shown_id(&m, SIDE);
    assert_ne!(unnamed, two);

    m.ok(|l| l.remove_output(SIDE));
    assert_eq!(named(&m, MAIN, "2"), two);
    assert!(
        m.layout
            .workspaces(MAIN)
            .unwrap()
            .iter()
            .any(|ws| ws.id == unnamed && ws.windows == [w(1)])
    );
    m.ok(|l| l.add_output(OWNER, SIDE, output(1920.0, 1920, 1080, 1.0)));
    assert_eq!(named(&m, SIDE, "2"), two);
    assert_eq!(
        shown_id(&m, SIDE),
        unnamed,
        "the one it showed is shown again"
    );

    // Without any output, the workspaces wait, ids kept.
    m.ok(|l| l.remove_output(SIDE));
    m.ok(|l| l.remove_output(MAIN));
    m.ok(|l| l.add_output(OWNER, MAIN, output(0.0, 1920, 1080, 1.0)));
    assert_eq!(named(&m, MAIN, "2"), two);
    assert!(
        m.layout
            .workspaces(MAIN)
            .unwrap()
            .iter()
            .any(|ws| ws.id == unnamed)
    );
}

/// A named workspace removed keeps its windows, and its id, unnamed.
#[test]
fn a_workspace_keeps_its_id_when_it_loses_its_name() {
    let mut m = Model::new();
    m.ok(|l| l.focus_workspace(OWNER, "a"));
    m.open(1);
    let a = named(&m, MAIN, "a");
    m.ok(|l| l.remove_workspace(OWNER, "a"));
    let list = m.layout.workspaces(MAIN).unwrap();
    let kept = list.iter().find(|ws| ws.windows == [w(1)]).unwrap();
    assert_eq!((kept.id, kept.name.as_deref()), (a, None));
}

/// `tidy` removing an empty unnamed workspace before the one shown moves the shown one's index,
/// not its id: nothing switched.
#[test]
fn an_unnamed_workspace_going_before_the_shown_one_leaves_its_id() {
    let mut m = Model::new();
    m.ok(|l| l.add_workspace(OWNER, "a", None));
    m.ok(|l| l.add_workspace(OWNER, "b", None));
    m.ok(|l| l.focus_workspace(OWNER, "a"));
    m.open(1);
    m.ok(|l| l.focus_workspace(OWNER, "b"));
    m.ok(|l| l.remove_workspace(OWNER, "a"));
    assert_eq!(m.workspaces(MAIN), (vec!["-".into(), "b".into()], 1));
    let b = shown_id(&m, MAIN);

    m.ok(|l| l.close_window(w(1)));
    assert_eq!(m.workspaces(MAIN), (vec!["b".into()], 0));
    assert_eq!(shown_id(&m, MAIN), b);
    assert_eq!(named(&m, MAIN, "b"), b);
}

/// A column keeps its id when it moves along the strip, changes width, takes a window from its
/// neighbour, and goes fullscreen and back; the column the window came from is gone.
#[test]
fn a_column_keeps_its_id_through_moves_widths_consumes_and_fullscreen() {
    let mut m = Model::new();
    for n in 1..=3 {
        m.open(n);
    }
    let ids = [column(&m, 1), column(&m, 2), column(&m, 3)];
    assert_ne!(ids[0], ids[1]);
    assert_ne!(ids[1], ids[2]);
    let unchanged = |m: &Model| [column(m, 1), column(m, 2), column(m, 3)] == ids;

    m.ok(|l| l.move_focused(OWNER, Direction::Left));
    m.ok(|l| l.move_focused(OWNER, Direction::Left));
    let order = &m.layout.workspaces(MAIN).unwrap()[0].windows;
    assert_eq!(order, &[w(3), w(1), w(2)]);
    assert!(unchanged(&m), "moving columns keeps their ids");
    m.ok(|l| l.move_focused(OWNER, Direction::Right));
    m.ok(|l| l.change_width(w(3), 0.1));
    m.ok(|l| l.cycle_preset_width(w(1), true));
    m.ok(|l| l.set_full_width(w(2), true));
    m.ok(|l| l.set_full_width(w(2), false));
    assert!(unchanged(&m), "changing widths keeps the ids");
    m.ok(|l| l.set_fullscreen(w(3), true));
    assert_eq!(column(&m, 3), ids[2]);
    m.ok(|l| l.set_fullscreen(w(3), false));
    assert!(unchanged(&m), "fullscreen in and out keeps the id");

    // Window 3, alone, joins window 1's column on its left: that column keeps its id.
    m.ok(|l| l.focus_window(w(3)));
    m.ok(|l| l.consume_or_expel(OWNER, Direction::Left));
    assert_eq!(column(&m, 3), ids[0]);
    assert_eq!(column(&m, 1), ids[0]);
    assert_eq!(column(&m, 2), ids[1]);
    assert!(
        m.placed(MAIN).iter().all(|p| p.column != Some(ids[2])),
        "window 3's own column is gone"
    );
}

/// A window expelled from its column gets a new column, and a column whose last window leaves is
/// gone for good: no later column takes its id.
#[test]
fn an_expelled_window_gets_a_new_column_and_a_column_gone_never_comes_back() {
    let mut m = Model::new();
    m.open(1);
    m.open(2);
    m.ok(|l| l.consume_or_expel(OWNER, Direction::Left));
    let shared = column(&m, 1);
    assert_eq!(column(&m, 2), shared);
    let mut seen = vec![shared];

    m.ok(|l| l.consume_or_expel(OWNER, Direction::Right));
    assert_eq!(column(&m, 1), shared, "the column left keeps its id");
    assert_new(&mut seen, column(&m, 2));
    m.ok(|l| l.close_window(w(2)));
    m.ok(|l| l.close_window(w(1)));
    for n in 3..=5 {
        m.open(n);
        assert_new(&mut seen, column(&m, n));
    }
    // A window moved to another workspace gets a new column there.
    m.ok(|l| l.move_to_workspace(OWNER, "other"));
    assert_new(&mut seen, column(&m, 5));
}

/// Dwindle has no columns; a workspace back to scrolling columns gets new ones.
#[test]
fn switching_to_dwindle_and_back_makes_new_columns() {
    let mut m = Model::new();
    m.open(1);
    m.open(2);
    let mut seen = vec![column(&m, 1), column(&m, 2)];
    m.ok(|l| l.set_tiling(OWNER, Tiling::Dwindle));
    for n in [1, 2] {
        let placed = m.placement(n);
        assert_eq!((placed.layer, placed.column), (Layer::Tiles, None));
    }
    m.ok(|l| l.set_tiling(OWNER, Tiling::Scrolling));
    assert_new(&mut seen, column(&m, 1));
    assert_new(&mut seen, column(&m, 2));
}

/// Each window says its workspace and its layer: tiled on a strip or in tiles, fullscreen in the
/// layer of its tiling, floating, pinned on the workspace shown; a special workspace's windows on
/// the special workspace.
#[test]
fn each_window_says_its_workspace_and_its_layer() {
    let mut m = Model::new();
    m.open(1);
    m.open(2);
    let main = shown_id(&m, MAIN);
    m.ok(|l| l.set_fullscreen(w(2), true));
    let placed = m.placement(2);
    assert!(placed.fullscreen);
    assert_eq!(
        (placed.workspace, placed.layer, placed.column),
        (main, Layer::Strip, Some(column(&m, 2)))
    );
    m.ok(|l| l.set_fullscreen(w(2), false));
    m.open_with(OWNER, 3, floating());
    m.open_with(OWNER, 4, floating());
    m.ok(|l| l.set_pinned(w(4), true));
    let layers: Vec<_> = m
        .placed(MAIN)
        .into_iter()
        .map(|p| (p.window.0, p.workspace, p.layer, p.column.is_some()))
        .collect();
    assert_eq!(
        layers,
        [
            (1, main, Layer::Strip, true),
            (2, main, Layer::Strip, true),
            (3, main, Layer::Floating, false),
            (4, main, Layer::Pinned, false),
        ]
    );

    m.ok(|l| l.toggle_special(OWNER, "scratch"));
    m.ok(|l| l.set_tiling(OWNER, Tiling::Dwindle));
    m.open(5);
    let special = m.layout.shown_workspaces(MAIN).unwrap().special.unwrap();
    assert_ne!(special.id, main);
    let drawn = m.placed(MAIN);
    let say = |n: u64| {
        let p = drawn.iter().find(|p| p.window == w(n)).unwrap();
        (p.workspace, p.layer, p.column.is_some())
    };
    assert_eq!(say(1), (main, Layer::Strip, true));
    assert_eq!(say(3), (main, Layer::Floating, false));
    assert_eq!(say(5), (special.id, Layer::Tiles, false));
    assert_eq!(
        say(4),
        (main, Layer::Pinned, false),
        "a pinned window is on the workspace shown, under the special one or not"
    );
}

/// The views of the workspaces an output shows: ids, cameras on the physical grid, none in
/// dwindle, and the working area, the whole output today.
#[test]
fn an_output_gives_the_camera_and_the_working_area_of_what_it_shows() {
    let mut m = Model::new();
    for n in 1..=3 {
        m.open(n);
    }
    let shown = m.layout.shown_workspaces(MAIN).unwrap();
    assert_eq!(shown.main.id, m.workspace_id(MAIN, 0));
    // The third column came in from the right, a gap from the output's edge.
    assert_eq!(shown.main.view_x, Some(3.0 * STEP - 1920.0));
    assert_eq!(shown.main.working, rect(0.0, 0.0, 1920.0, 1080.0));
    assert_eq!(shown.special, None);

    m.ok(|l| l.toggle_special(OWNER, "scratch"));
    m.ok(|l| l.set_tiling(OWNER, Tiling::Dwindle));
    let special = m.layout.shown_workspaces(MAIN).unwrap().special.unwrap();
    assert_eq!(special.view_x, None);
    assert_eq!(special.working, rect(0.0, 0.0, 1920.0, 1080.0));

    let unknown = OutputId(9);
    assert_eq!(
        m.layout.shown_workspaces(unknown),
        Err(LayoutError::UnknownOutput(unknown))
    );
}

/// Focusing another column moves the camera alone: every window keeps its strip position, and
/// its screen position is that minus the camera.
#[test]
fn focusing_another_column_moves_the_camera_alone() {
    let mut m = Model::new();
    for n in 1..=3 {
        m.open(n);
    }
    let strip = |m: &Model| [1, 2, 3].map(|n| strip_x(m, n));
    assert_eq!(strip(&m), [0.0, STEP, 2.0 * STEP]);
    let camera = view_x(&m, MAIN);

    for direction in [Direction::Left, Direction::Left] {
        m.ok(|l| l.focus(OWNER, direction));
        assert_eq!(strip(&m), [0.0, STEP, 2.0 * STEP]);
    }
    assert_eq!(m.focused(), Some(1));
    assert_eq!(
        view_x(&m, MAIN),
        -16.0,
        "column 1 came in a gap from the left edge"
    );
    assert_ne!(view_x(&m, MAIN), camera);
    for n in 1..=3 {
        assert_eq!(m.rect(n).x, strip_x(&m, n) - view_x(&m, MAIN));
    }
    for direction in [Direction::Right, Direction::Right] {
        m.ok(|l| l.focus(OWNER, direction));
        assert_eq!(strip(&m), [0.0, STEP, 2.0 * STEP]);
    }
    assert_eq!(view_x(&m, MAIN), camera);
}

/// A column that comes in left of the focused one moves the columns on its right along the strip
/// by its width and a gap, and the camera by as much: those columns stay where they are on
/// screen. Today a column comes in there one way: a window of a column on the left goes
/// fullscreen, and leaves its column for one of its own. The columns left of the new one keep
/// their strip positions, so the camera's move takes them across the screen. Closing that window
/// undoes it all.
#[test]
fn a_column_coming_in_left_of_the_focused_one_moves_the_strip_and_the_camera_together() {
    let mut m = Model::new();
    m.open(1);
    m.open(2);
    m.ok(|l| l.consume_or_expel(OWNER, Direction::Left));
    m.open(3);
    m.open(4);
    // Columns: (1, 2) at 0, 3 at one step, 4 at two; the view shows 3 and 4.
    let shared = column(&m, 2);
    let strip_before = [2, 3, 4].map(|n| strip_x(&m, n));
    assert_eq!(strip_before, [0.0, STEP, 2.0 * STEP]);
    let screen_before = [2, 3, 4].map(|n| m.rect(n).x);
    let camera_before = view_x(&m, MAIN);

    m.ok(|l| l.set_fullscreen(w(1), true));
    assert_eq!(m.focused(), Some(4));
    assert_eq!(
        column(&m, 2),
        shared,
        "the column window 1 left keeps its id"
    );
    assert!(![shared, column(&m, 3), column(&m, 4)].contains(&column(&m, 1)));
    // The new column is fullscreen: as wide as the output.
    let shift = 1920.0 + 16.0;
    assert_eq!(strip_x(&m, 2), strip_before[0], "left of the new column");
    assert_eq!(strip_x(&m, 3), strip_before[1] + shift);
    assert_eq!(strip_x(&m, 4), strip_before[2] + shift);
    assert_eq!(view_x(&m, MAIN), camera_before + shift);
    assert_eq!(m.rect(3).x, screen_before[1]);
    assert_eq!(m.rect(4).x, screen_before[2]);
    assert_eq!(m.rect(2).x, screen_before[0] - shift);

    m.ok(|l| l.close_window(w(1)));
    assert_eq!([2, 3, 4].map(|n| strip_x(&m, n)), strip_before);
    assert_eq!(view_x(&m, MAIN), camera_before);
    assert_eq!([2, 3, 4].map(|n| m.rect(n).x), screen_before);
}

/// A peek names its workspace by id: an unknown id, one gone, the special workspace shown over
/// the output and another output's workspace are refused.
#[test]
fn a_peek_refuses_a_workspace_not_among_the_outputs_own() {
    let mut m = Model::new();
    m.ok(|l| l.add_output(OWNER, SIDE, output(1920.0, 1920, 1080, 1.0)));
    m.ok(|l| l.focus_workspace(OWNER, "a"));
    m.open(1);
    let a = named(&m, MAIN, "a");
    assert_eq!(m.layout.arrange_workspace(MAIN, a).unwrap()[0].workspace, a);
    m.ok(|l| l.remove_workspace(OWNER, "a"));
    m.ok(|l| l.move_to_workspace(OWNER, "b"));
    m.ok(|l| l.focus_workspace(OWNER, "c"));
    assert!(
        m.layout
            .workspaces(MAIN)
            .unwrap()
            .iter()
            .all(|ws| ws.id != a),
        "the unnamed workspace emptied is gone"
    );
    m.ok(|l| l.toggle_special(OWNER, "scratch"));
    let special = m.layout.shown_workspaces(MAIN).unwrap().special.unwrap().id;
    let side = m.workspace_id(SIDE, 0);
    for workspace in [WorkspaceId(0), WorkspaceId(u64::MAX), a, special, side] {
        assert_eq!(
            m.layout.arrange_workspace(MAIN, workspace),
            Err(LayoutError::InvalidValue),
            "{workspace:?}"
        );
    }
}

/// A column's width set by a rule keeps the column's id when the window's client commits another
/// size: a commit moves no column to a new one.
#[test]
fn a_commit_keeps_the_column() {
    let mut m = Model::new();
    m.open_with(
        OWNER,
        1,
        OpenOptions {
            width: Some(ColumnWidth::Fixed(500.0)),
            ..OpenOptions::default()
        },
    );
    let id = column(&m, 1);
    m.ok(|l| {
        l.commit_size(
            w(1),
            yuna_layout::geometry::Size {
                w: 700.0,
                h: 1048.0,
            },
        )
    });
    assert_eq!(column(&m, 1), id);
}

#[test]
fn a_special_workspace_keeps_its_id_hidden_shown_again_and_on_another_output() {
    let mut m = Model::new();
    m.ok(|l| l.add_output(OWNER, SIDE, output(1920.0, 1920, 1080, 1.0)));
    m.ok(|l| l.focus_output(OWNER, MAIN));
    m.open(1);
    m.ok(|l| l.move_to_special(OWNER, "scratch"));
    m.ok(|l| l.toggle_special(OWNER, "scratch"));
    let first = m.layout.shown_workspaces(MAIN).unwrap().special.unwrap().id;
    let column_before = column(&m, 1);
    m.ok(|l| l.toggle_special(OWNER, "scratch"));
    assert_eq!(m.layout.shown_workspaces(MAIN).unwrap().special, None);
    m.ok(|l| l.toggle_special(OWNER, "scratch"));
    assert_eq!(
        m.layout.shown_workspaces(MAIN).unwrap().special.unwrap().id,
        first
    );
    m.ok(|l| l.focus_output(OWNER, SIDE));
    m.ok(|l| l.toggle_special(OWNER, "scratch"));
    assert_eq!(
        m.layout.shown_workspaces(SIDE).unwrap().special.unwrap().id,
        first
    );
    assert_eq!(column(&m, 1), column_before);
    m.check();
}
