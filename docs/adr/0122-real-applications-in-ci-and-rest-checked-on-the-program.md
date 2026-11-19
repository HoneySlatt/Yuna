# 0122. Real applications in CI, and rest checked on the program

- Status: Accepted; supersedes [0025](0025-applications-in-ci.md),
  [0029](0029-the-rest-of-animated-applications.md) (the rest of animated applications,
  SlimeEngine's workflow), [0043](0043-text-input-and-the-rest-of-slimeengines-examples.md)
  (SlimeEngine's examples at rest) and [0050](0050-mpv-in-the-applications-tests.md); supersedes
  in part [0104](0104-stage-1s-exit-opium-in-aeriths-place-and-its-exceptions-written.md) (Opium's weekly run)
- Date: 2026-11-18

## Context

Which real applications Yuna's tests run, and how their rest is checked, was decided in four ADRs.
[ADR-0025](0025-applications-in-ci.md) ran real applications in CI and checked rest on the `yuna`
program. [ADR-0029](0029-the-rest-of-animated-applications.md) handled applications that draw
without end and gave SlimeEngine's job a workflow of its own.
[ADR-0043](0043-text-input-and-the-rest-of-slimeengines-examples.md) checked SlimeEngine's
`editor` at rest in place of its `window`. [ADR-0050](0050-mpv-in-the-applications-tests.md)
added mpv. [ADR-0104](0104-stage-1s-exit-opium-in-aeriths-place-and-its-exceptions-written.md) put Opium in Aerith's place in Stage 1's exit. The ROADMAP cited
ADR-0025 alone.

This ADR gathers the rule as it stands on 2026-11-18. It decides nothing new. The quality bar
measures rest by every thread's context switches over 10 s
([ROADMAP](../ROADMAP.md#3-quality-bar)).

## Decision

- **Every push runs real applications in Yuna headless**, in CI's job (ADR-0025, 2026-11-12):
  Alacritty, a GTK 4 and a Qt 6 application, `vkcube` for Vulkan through Mesa, and mpv (ADR-0050,
  2026-11-14), at the versions Yuna's `flake.lock` pins.
- **mpv runs in Vulkan on lavapipe**, with no configuration and no sound, on a picture it makes
  itself (`av://lavfi:testsrc2`), so that no file is fetched (ADR-0050). Two cases prove it read
  Yuna's colour description: an SDR video on an HDR output, a PQ video on an SDR output. mpv's
  surface then declares what it derived from the output's description: the HDR output's monitor
  volume as its target, or the SDR output's gamma 2.2 and minimum luminance, scaled to its own
  reference white of 203 cd/m². Yuna rests after mpv quits.
- **Rest is checked on the `yuna` program**, as agents run it, not in the test's own process
  (ADR-0025). The test waits until each application has shown its window and gone quiet, then
  requires 10 s with no context switch in any of Yuna's threads, and no frame, frame callback,
  configure or commit. What makes an application redraw is pinned: fonts, cursor blinking,
  renderers, accessibility.
- **Applications that draw without end** (ADR-0029, 2026-11-12; ADR-0043, 2026-11-13). `vkcube`
  and SlimeEngine's `window` draw 120 frames and leave: Yuna must get a commit for each, draw them,
  and rest 10 s once its client left. `vkcube`, paused by its space key through `yuna mcp`, is
  checked at rest with its window open, in CI. `vkcube` left to draw is the proof that a client
  that keeps drawing fails the rest check.
- **SlimeEngine's `window` cannot rest with its window shown**, by its design: it stays checked
  after it leaves. **SlimeEngine's `editor` is checked at rest instead**, its window shown: a
  SlimeEngine client on wgpu and Vulkan that rests. Nothing changes in SlimeEngine (ADR-0043).
- **Protocol tests and misbehaving clients run in the test's process** (ADR-0025), written with
  `wayland-client`: clients that ignore their size, send absurd values, break the protocol or leave
  mid-frame, and random sequences of requests with proptest.
- **SlimeEngine's examples run in a workflow of their own**, `.github/workflows/slimeengine.yml`:
  weekly, on demand, and on a change of the drawing path (ADR-0025; ADR-0029). They are built at a
  SlimeEngine revision pinned in Yuna, their binary cached per revision. `window`, `editor`, and
  `interface`, whose bar holds buttons and a text field, run there, the last two driven through
  `yuna mcp`, since they need SlimeEngine's cooked data (ADR-0043). Aerith's surface is tested
  there when its relay passes colours on (ADR-0050).
- **Stage 1's exit**: until uwui has windowed examples, SlimeEngine's take their place (ADR-0025).
  Opium's desktop program, built from its Nix package at a revision Yuna pins, takes Aerith's
  place, which has no window, in the same weekly job: an agent, through `yuna mcp` alone, opens it
  on a throwaway profile, finds its window, acts on it and checks it by capture (ADR-0104,
  2026-11-18). The exit's exceptions stay ADR-0104's.

## Alternatives considered

- **SlimeEngine's examples at every push**, or **by hand only** ([ADR-0025](0025-applications-in-ci.md)):
  CI time at each push and a long first build of each revision, or no regression caught between
  two runs.
- **Rest checked in the test's process** ([ADR-0025](0025-applications-in-ci.md)): Mesa's and the
  harness's threads mix in, and it is not what agents run.
- **Real applications only, without `wayland-client`** ([ADR-0025](0025-applications-in-ci.md)): a
  real application never breaks the protocol on demand.
- **A number of frames as the animated applications' whole check**
  ([ADR-0029](0029-the-rest-of-animated-applications.md)): a Vulkan client that rests with its
  window open would never be checked.
- **SlimeEngine's job in CI's workflow** ([ADR-0029](0029-the-rest-of-animated-applications.md)): a
  path filter applies to a whole workflow, so one job would run too often or the other too
  seldom.
- **An on-demand option in SlimeEngine's `window`**
  ([ADR-0043](0043-text-input-and-the-rest-of-slimeengines-examples.md)): a change in another
  project, where a session is working.
- **mpv in the weekly job only** ([ADR-0050](0050-mpv-in-the-applications-tests.md)): a regression
  shows a week later.

## Consequences

- CI fetches about 0.7 GB of applications and mpv's 348 MB, without a cache of the Nix store.
- The rest check costs at least 10 s per application.
- Yuna's CI builds SlimeEngine and Opium at their pinned revisions once a week and on demand; a
  revision is raised by hand.

## Validation and reconsideration

Evidence: the applications' tests in CI and the weekly job's runs. Proposed again when uwui's own
windowed examples can take SlimeEngine's and Opium's place.
