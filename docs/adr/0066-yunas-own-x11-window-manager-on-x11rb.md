# 0066. Yuna's own X11 window manager, on x11rb

- Status: Accepted; settles what [0039](0039-leaving-smithay-step-by-step.md) left to M1.8's design
- Date: 2026-11-14

## Context

XWayland is an X server whose windows Yuna shows. It needs an X11 window manager: the program
that tells each X11 window where it is, its size, its state and whether it has the focus.
[ADR-0039](0039-leaving-smithay-step-by-step.md) left to M1.8's design whether Yuna writes its own
or uses Smithay's. What M1.8's design found (sources in the [survey](../survey.md#xwayland)):

- Smithay's `xwayland` module is 8,168 lines at Yuna's pin. It starts Xwayland at once, never on
  the first connection; it stamps Xwayland's client with its own data, so Yuna could not record
  the client's seat; it takes `WM_S0` before any setting exists, so the first client could read a
  wrong scale; it waits on X11 replies on the compositor's thread; three of its `unwrap`s are
  reachable by an X11 client's messages (`xwm/mod.rs:2463`, `:2755`, `:2800`); its open issues
  name a deadlock when Xwayland crashes and leaks. Under ADR-0039 it would leave before 0.2.
- xwayland-satellite 0.8.3, which niri runs, gives X11 the *smallest* output scale when scales
  differ, against [ADR-0044](0044-xwayland-across-outputs-of-different-scales.md); its
  supersampling is an unmerged pull request; it has no drag and drop from Wayland to X11.
- x11rb 0.14 is the X11 protocol in Rust that Smithay itself uses: its `RustConnection` is pure
  Rust, without libxcb, under MIT or Apache-2.0.

## Decision

The owner chose, on 2026-11-14: **Yuna writes its own X11 window manager, on x11rb's
`RustConnection`.**

- x11rb is a new direct dependency of `yuna`, with only the extensions the manager uses; it is a
  dev-dependency too, for the tests' X11 client.
- Each seat's manager waits on X11 replies on a thread of its own, which posts what it learned to
  Yuna's loop. Yuna's loop never waits on X11: a slow or stuck Xwayland never holds a frame, the
  owner's included ([ADR-0007](0007-surviving-a-crash.md)).
- Nothing an X11 client sends can panic Yuna; a panic in a manager's thread ends that seat's
  Xwayland, not Yuna.
- Smithay's module is read as a reference; under MIT, a passage close to it is credited in a
  comment ([ADR-0039](0039-leaving-smithay-step-by-step.md)).
- What M1.8 handles: managed and override-redirect windows; reparenting decided in the code's
  documentation; `WM_STATE`; `WM_NAME`, `_NET_WM_NAME`, `WM_CLASS` and their encodings; size
  hints; `WM_TRANSIENT_FOR`; window types; `WM_DELETE_WINDOW`; `_NET_WM_STATE` fullscreen,
  maximized and hidden; focus with `WM_TAKE_FOCUS` and `_NET_ACTIVE_WINDOW`; stacking;
  `_NET_WM_SYNC_REQUEST` with a timeout; `_XWAYLAND_ALLOW_COMMITS`; `_GTK_FRAME_EXTENTS`;
  `_MOTIF_WM_HINTS`; `_NET_WM_MOVERESIZE`; `_NET_STARTUP_ID`; pairing a window with its surface
  through `xwayland-shell-v1`, in either order; XSETTINGS and `RESOURCE_MANAGER`.
- What waits: the clipboard and drag and drop between X11 and Wayland, at M1.11; X11 keyboard
  grabs (`xwayland-keyboard-grab`), at Stage 2.

## Alternatives considered

- **Smithay's manager, patched in the fork**: fastest to wire, and complete, drag and drop
  included. But on-demand start, seats and the order of the start would each need Yuna's code
  around it or patches in it, and the whole would be rewritten before 0.2.
- **xwayland-satellite as an external program**: no X11 code in Yuna, but it fails ADR-0044
  today, and Yuna would control neither the scale nor games' fullscreen.

## Consequences

- About 7,000 to 9,000 lines with their tests, a large part of M1.8.
- An X11 window joins the layout and the motion as an `xdg_toplevel` does: one record of
  windows, two shells behind it.
- Each Smithay item Yuna's manager does not use stays out of Yuna; when Yuna's core moves at
  Stage 2, the manager is unchanged.
