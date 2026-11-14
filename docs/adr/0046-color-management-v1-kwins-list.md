# 0046. `color-management-v1`: Yuna's own handler, KWin's list

- Status: Accepted
- Date: 2026-11-14

## Context

`color-management-v1` (wayland-protocols staging, version 3) lets a client describe its surface's
colours and read its output's. Smithay has no handler for it; it is Yuna's first handler of its
own on wayland-server ([ADR-0039](0039-leaving-smithay-step-by-step.md)). Each feature, transfer
function and set of primaries a compositor lists is a promise its colour pipeline keeps at
Stage 5. What the clients that matter need, read on 2026-11-14:

- mpv 0.42 uses the protocol only with `parametric`; it reads its surface's preferred
  description to pick its target. Chromium needs `parametric`.
- Mesa's Vulkan WSI binds version 1 and exposes a Vulkan colour space when its primaries and
  transfer function are listed; it sends HDR metadata through `set_mastering_display_primaries`,
  and, with `extended_target_volume` listed, turns any refused description into a lost surface.
- The transfer function `srgb` is deprecated from version 2. Listed to version-1 clients, as
  Mutter does, it makes Mesa take a colour surface for every sRGB swapchain, on a `wl_surface`
  that GTK or mpv may already have taken one for: the protocol error `surface_exists`, which
  ends the client. KWin never lists it; GTK then leaves colour management off, which changes
  nothing for SDR content.
- The protocol's only polite refusal, `failed`, ends a Mesa client.

## Decision

The owner chose, on 2026-11-14, KWin's list, without ICC:

- Version 3. Features: `parametric`, `set_primaries`, `set_luminances`,
  `set_mastering_display_primaries`. Transfer functions: `gamma22`, `bt1886`, `st2084_pq`,
  `hlg`, `ext_linear`, `compound_power_2_4`. The ten named primaries. The perceptual intent
  alone.
- Not listed: `srgb`, `extended_target_volume`, `set_tf_power`, ICC profiles, Windows' scRGB
  and BT.2100. Each comes with the stage that renders it, in an ADR.
- Every event and value is sent only at the version the client bound. Every rule of the
  protocol is checked but the one below, read as KWin reads it: a 0 for `max_cll`, `max_fall` or a mastering
  maximum means unset; invalid mastering primaries are dropped, not refused. Sums are computed
  in 64 bits, products of coordinates in 128, so that no client's number overflows.
- The handler is Yuna's code on wayland-server's `Dispatch`, not Smithay's dispatch macros. A
  surface's description is double-buffered in Smithay's cached state until Yuna's own core
  holds surfaces.
- Version 1's further checks of `max_cll` and `max_fall` against the target range are not made:
  version 2 drops them, and Mesa, which binds version 1, sends HDR metadata it does not check
  against them (`wsi_common_wayland.c`, read 2026-11-14); a refusal there would end the game.
- Each description lives in its protocol object; Yuna keeps no registry of them, and no cap of
  [ADR-0028](0028-bounds-on-what-one-client-may-ask.md) applies: the protocol's only refusal
  ends Mesa's clients. wayland-server bounds no count of objects; a client's descriptions cost
  it its own objects, and its feedback objects cost Yuna a map entry each.

## Alternatives considered

- **KWin's list and `srgb` for version-1 clients**, as Mutter: GTK and Mesa's sRGB swapchains
  would use the protocol, at the risk of ending them.
- **ICC profiles too**: an ICC reader to keep (lcms2, a C library), for no client M1.6 needs.

## Consequences

- Stage 5's pipeline honours six transfer functions, ten primaries and arbitrary primaries.
- Tests drive the protocol at versions 1, 2 and 3 through Yuna's own client.
