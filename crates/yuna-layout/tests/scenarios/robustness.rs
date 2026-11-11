//! Bad input: stale ids, nonsense numbers, duplicates. Every one is an error that changes
//! nothing, never a panic (ADR-0007).

use crate::harness::*;
use yuna_layout::geometry::Point;
use yuna_layout::{
    ColumnWidth, Direction, LayoutError, OpenOptions, OutputConfig, OutputId, SeatId,
};

#[test]
fn stale_ids_are_refused() {
    let mut m = Model::new();
    m.open(1);
    m.ok(|l| l.close_window(w(1)));
    m.refused(LayoutError::UnknownWindow(w(1)), |l| l.close_window(w(1)));
    m.refused(LayoutError::UnknownWindow(w(1)), |l| {
        l.set_fullscreen(w(1), true)
    });
    m.refused(LayoutError::UnknownSeat(SeatId(9)), |l| {
        l.focus(SeatId(9), Direction::Left)
    });
    m.refused(LayoutError::UnknownOutput(OutputId(9)), |l| {
        l.remove_output(OutputId(9))
    });
    m.refused(LayoutError::UnknownWorkspace("x".into()), |l| {
        l.remove_workspace(OWNER, "x")
    });
    assert!(m.layout.window(w(1)).is_none());
    assert!(m.layout.arrange(OutputId(9)).is_err());
}

#[test]
fn duplicate_ids_are_refused() {
    let mut m = Model::new();
    m.open(1);
    m.refused(LayoutError::AlreadyExists, |l| {
        l.open_window(OWNER, w(1), OpenOptions::default())
    });
    m.refused(LayoutError::AlreadyExists, |l| l.add_seat(OWNER));
    m.refused(LayoutError::AlreadyExists, |l| {
        l.add_output(OWNER, MAIN, output(0.0, 800, 600, 1.0))
    });
    m.ok(|l| l.add_workspace(OWNER, "a", None));
    m.refused(LayoutError::AlreadyExists, |l| {
        l.add_workspace(OWNER, "a", None)
    });
}

#[test]
fn nonsense_numbers_are_refused() {
    let mut m = Model::new();
    m.open(1);
    let bad_outputs = [
        output(0.0, 1920, 1080, 0.0),
        output(0.0, 1920, 1080, f64::NAN),
        output(0.0, 1920, 1080, 1e9),
        output(0.0, 0, 1080, 1.0),
        OutputConfig {
            position: Point {
                x: f64::INFINITY,
                y: 0.0,
            },
            ..output(0.0, 1920, 1080, 1.0)
        },
    ];
    for config in bad_outputs {
        m.refused(LayoutError::InvalidValue, |l| {
            l.add_output(OWNER, SIDE, config)
        });
    }
    for width in [
        ColumnWidth::Proportion(f64::NAN),
        ColumnWidth::Proportion(0.0),
        ColumnWidth::Proportion(2.0),
        ColumnWidth::Fixed(-1.0),
        ColumnWidth::Fixed(f64::INFINITY),
    ] {
        m.refused(LayoutError::InvalidValue, |l| {
            l.open_window(
                OWNER,
                w(2),
                OpenOptions {
                    width: Some(width),
                    ..OpenOptions::default()
                },
            )
        });
    }
    m.refused(LayoutError::InvalidValue, |l| {
        l.change_width(w(1), f64::NAN)
    });
    m.ok(|l| l.set_floating(w(1), true));
    m.refused(LayoutError::InvalidValue, |l| {
        l.set_floating_rect(w(1), rect(f64::NAN, 0.0, 10.0, 10.0))
    });
    m.refused(LayoutError::InvalidValue, |l| {
        l.set_floating_rect(w(1), rect(0.0, 0.0, 0.0, 10.0))
    });
}

#[test]
fn a_thousand_windows_open_and_close_with_every_invariant_kept() {
    let mut m = Model::new();
    for n in 0..1000 {
        m.layout
            .open_window(OWNER, w(n), OpenOptions::default())
            .unwrap();
    }
    m.check();
    assert_eq!(m.focused(), Some(999));
    for n in (0..1000).step_by(3) {
        m.layout.close_window(w(n)).unwrap();
    }
    m.ok(|l| l.set_tiling(OWNER, yuna_layout::Tiling::Dwindle));
    m.ok(|l| l.set_tiling(OWNER, yuna_layout::Tiling::Scrolling));
}
