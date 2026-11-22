//! Yuna's shared vocabulary: the ids of what the compositor names, the directions and tilings a
//! caller asks for, and a region of an output's pixels.
//!
//! This crate is level 0 of the crate map
//! ([ADR-0082](../../../docs/adr/0082-the-requirements-of-stage-2s-core-its-bets-made-rules-or-deferred.md)'s
//! table of crates, as restated by
//! [ADR-0150](../../../docs/adr/0150-the-crate-map-yuna-wlcs-owns-wlcss-interface.md)):
//! it depends on nothing, and every crate of the map may depend on it.
//!
//! # What it promises
//!
//! - Plain data: every type is `Copy`, and compares and hashes by value.
//! - No dependency, no behaviour: a type here is a name or a choice, never a function of other
//!   state. The models that hand out ids keep that logic (the layout gives workspace and column
//!   ids).
//! - No `unsafe` code.
//!
//! # What comes here
//!
//! A type comes when more than one crate needs it, not before. The rest of level 0 (coordinate
//! spaces, transforms, the scale in 120ths, time values, dma-buf descriptors) comes with the code
//! that needs it. No crate re-exports these types: each names `yuna_types` itself.

#![forbid(unsafe_code)]

use std::num::NonZeroU64;

/// A window, named by the caller. The caller keeps ids unique among the windows it opens.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct WindowId(pub u64);

/// An output, named by the caller. The caller gives the same id to the same monitor each time it
/// is plugged in, so that its workspaces come back to it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct OutputId(pub u64);

/// A seat: a pointer and a keyboard with their own focus, the owner's or an agent's.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SeatId(pub u64);

/// A workspace's id, which the layout model gives every workspace it creates: named, unnamed or
/// special. Ids start at 1 and are never reused for the life of a layout, across seats. A
/// workspace keeps its id when it moves to another output and back, and when it loses its name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct WorkspaceId(pub u64);

/// A column's id, which the layout model gives every column of scrolling columns it creates. Ids
/// start at 1 and are never reused for the life of a layout, across seats and workspaces. A column
/// keeps its id while it exists: when it moves along its strip, a window joins or leaves it, it
/// changes width, or it goes fullscreen and back. A window that leaves it for a column of its own
/// gets a new column, with a new id; a column whose last window leaves is gone. A workspace whose
/// tiling changes to dwindle and back gets new columns.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ColumnId(pub NonZeroU64);

/// A direction on screen. Workspaces of an output are stacked vertically: up is the previous one,
/// down the next.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Direction {
    /// Towards the left.
    Left,
    /// Towards the right.
    Right,
    /// Upwards.
    Up,
    /// Downwards.
    Down,
}

/// How a workspace tiles its windows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Tiling {
    /// Columns on a strip that scrolls horizontally (niri).
    Scrolling,
    /// Each new window splits the focused one's space (Hyprland).
    Dwindle,
}

/// A region of an output's buffer, in physical pixels from its top-left corner: unsigned, so
/// that a region starts on the buffer or past it, never before it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PixelRect {
    /// The left edge.
    pub x: u32,
    /// The top edge.
    pub y: u32,
    /// The width.
    pub width: u32,
    /// The height.
    pub height: u32,
}

impl PixelRect {
    /// The one pixel at (`x`, `y`).
    pub fn pixel(x: u32, y: u32) -> PixelRect {
        PixelRect {
            x,
            y,
            width: 1,
            height: 1,
        }
    }
}
