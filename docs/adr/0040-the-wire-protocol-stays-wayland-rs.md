# 0040. The wire protocol stays wayland-rs's

- Status: Accepted
- Date: 2026-11-13

## Context

[ADR-0039](0039-leaving-smithay-step-by-step.md) has Yuna write its own base above the wire
protocol. The owner first asked for everything, as jay does: the wire protocol, code generated from
the protocols' XML, and the event loop written by Yuna too
([survey](../survey.md#an-own-base-and-vulkan)).

- Every C and C++ compositor (Mutter, KWin, wlroots, Weston, Mir, Louvre, Hyprland) writes its own
  protocol handlers and links libwayland-server. Hyprland replaced wlroots in 2024 and kept it:
  "it's only the protocol 'language'". jay alone writes its wire; in 2022 its author refused
  wayland-rs because no compositor anyone used daily was built on it. niri and cosmic-comp are now.
- wayland-server (wayland-rs, MIT) already filters globals per client (`GlobalDispatch::can_view`,
  enforced at bind and announcement by wayland-backend), bounds each client's buffer at 4,096
  bytes and its file descriptors at 28 a message, and runs its tests in four pairings against
  libwayland. It can also run on libwayland itself.
- Its costs are known and small: a message of more than four arguments allocates when dispatched;
  in a steady frame, only `wp_presentation_feedback.presented` has more. Yuna already rests with
  no context switch on calloop ([measurements](../measurements.md)). No one has measured a cost
  of wayland-rs against a budget.
- wayland-rs, calloop and drm-rs belong to the Smithay organisation, but carry no AI policy
  (Smithay's `AI.md` covers contributions to Smithay only), and Yuna contributes to none.

## Decision

The owner chose, on 2026-11-13:

- **Yuna keeps wayland-server and wayland-backend, calloop and drm-rs.** It writes everything
  above them: the protocol handlers, the scene, the renderer, DRM and KMS logic, input and the
  session.
- **Yuna writes its own wire protocol only on a measure**: a profile, on the laptop, of wire and
  dispatch with many clients that shows it costs a budget, or a need wayland-rs refuses. Such a
  wire would then follow wayland-server's API, so that the move is mechanical, and be tested
  against libwayland.
- The system's C libraries stay linked, never rewritten: libinput, libxkbcommon, libgbm,
  libudev, the Vulkan loader. Their bindings (Smithay's organisation's crates or Yuna's own FFI)
  are decided by the milestone that first needs them.

## Alternatives considered

- **Everything Yuna's own, as jay** (the owner's first scope): full independence, but about 5,000
  more lines, a parser of untrusted messages to fuzz and keep ourselves, for gains no one has
  measured.

## Consequences

- Wire-level bugs and limits are wayland-rs's; a patch, if one is ever needed, goes to a fork of
  it under the same rules as Smithay's ([ADR-0021](0021-smithays-fork.md)).
- The gap of [ADR-0035](0035-agents-seats.md) is Smithay's, not wayland-server's: its seat and
  output globals keep the default `can_view`. [ADR-0042](0042-main-on-smithays-fork.md) closes it.
- Fuzzing Yuna's handlers through wayland-backend stays possible, and needs its own ADR: it needs a
  nightly toolchain ([ADR-0016](0016-the-rust-toolchain.md)).
