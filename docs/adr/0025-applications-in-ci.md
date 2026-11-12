# 0025. Real applications in CI, and rest checked on the program

- Status: Accepted; amended by [0029](0029-the-rest-of-animated-applications.md) (when an
  animated application's rest is checked)
- Date: 2026-11-12

## Context

M1.4 is done when real applications run in Yuna in CI and draw nothing at rest. The bar measures
rest by every thread's context switches over 10 s ([ROADMAP](../ROADMAP.md#3-quality-bar)).
Yuna's repository is private, so its CI minutes are paid
([ADR-0009](0009-gpl-private-until-ready.md)). On 2026-11-12:

- uwui has no code yet: its repository holds documents and a logo, and its code comes at
  SlimeEngine's M6d.11. The windowed examples that M1.4, M1.5 and Stage 1's exit name do not exist.
- SlimeEngine is public. Its `window` example draws through wgpu and Vulkan. Its own CI runs a
  series of headless compositors in a job of its own, on change, weekly and on demand
  (SlimeEngine's ADR-0195).
- Alacritty 0.17, GTK 4.24's `gtk4-demo`, Qt 6.12's `qml` and Mesa 26.2 are in the nixpkgs that
  Yuna's `flake.lock` pins: about 0.7 GB to download.

## Decision

- **Every push** runs Alacritty, a GTK 4 and a Qt 6 application, and `vkcube` for Vulkan through
  Mesa, in Yuna headless, in CI's one job.
- **Rest is checked on the `yuna` program**, as agents run it, not in the test's own process.
  The test waits until each application has shown its window and gone quiet. It then requires 10 s
  with no context switch in any of Yuna's threads, and no frame, frame callback, configure or
  commit. A client that keeps drawing must make the same check fail. What makes an application
  redraw is pinned: fonts, cursor blinking, renderers, accessibility.
- **Protocol tests and misbehaving clients run in the test's process**, written with
  `wayland-client`: clients that ignore their size, send absurd values, break the protocol or leave
  mid-frame, and random sequences of requests with proptest.
- **SlimeEngine's `window` example** runs in a second job, weekly, on demand, and when the backend
  changes. It is built at a SlimeEngine revision pinned in Yuna, and its binary is cached per
  revision.
- **Until uwui has windowed examples, SlimeEngine's take their place** in M1.4's and M1.5's
  done-when and in Stage 1's exit.

## Alternatives considered

- **SlimeEngine's example at every push**, its binary cached: every regression seen at once, at the
  cost of CI time at each push and a long first build of each revision.
- **SlimeEngine's example run by hand only**: no CI minutes, but nothing catches a regression
  between two runs.
- **Rest checked in the test's process**: simpler, but Mesa's and the test harness's threads mix
  in, and it is not what agents run.
- **Real applications only**, without `wayland-client`: no new crate, but a real application never
  breaks the protocol on demand, so [ADR-0007](0007-surviving-a-crash.md)'s robustness would go
  untested.

## Consequences

- CI gains about 0.7 GB of downloads and Smithay's build; its time is measured with M1.4's first
  Smithay commit, and caching is put to the owner if it grows too much.
- The rest check costs at least 10 s per application.
- The workflow gets a second job.
