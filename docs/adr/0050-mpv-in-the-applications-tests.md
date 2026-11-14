# 0050. mpv in the applications' tests

- Status: Accepted; amends [0025](0025-applications-in-ci.md) (one more application in CI)
- Date: 2026-11-14

## Context

M1.6 is done when a test reads the colour description Aerith's or mpv's surface sent. Aerith's
relay does not pass `color-management-v1` on yet (its own M1.5). mpv 0.42 uses the protocol in
OpenGL, Vulkan and `dmabuf-wayland`, and by default copies its surface's preferred description
as its target: an HDR video on an HDR output would look right whether or not mpv read Yuna's
description. nixpkgs' mpv 0.42.0 adds 348 MB, 52 store paths, to what the development shell
already fetches (measured 2026-11-14).

## Decision

The owner chose, on 2026-11-14:

- mpv joins the applications the tests run on every push
  ([ADR-0025](0025-applications-in-ci.md)), in Vulkan on lavapipe, with no configuration and no
  sound, on a picture it makes itself (`av://lavfi:testsrc2`), so that no file is fetched.
- Two cases prove it read Yuna's description: an SDR video on an HDR output, and a PQ video on
  an SDR output. mpv's surface then declares what it derived from the output's description:
  the HDR output's monitor volume as its target, or the SDR output's gamma 2.2 and minimum
  luminance, scaled to its own reference white of 203 cd/m². Yuna rests after mpv quits.
- Aerith's own surface is tested when its relay passes colours on, in the weekly job.

## Alternatives considered

- **The weekly job only**, as SlimeEngine's examples: CI per push stays smaller, a regression
  shows a week later.

## Consequences

- CI's jobs fetch 348 MB more, without a cache of the Nix store.
