# 0029. The rest of animated applications, and the buffers CI's applications use

- Status: Accepted; amends [0025](0025-applications-in-ci.md) (when an animated application's
  rest is checked) and [0022](0022-the-headless-backend-in-yuna.md) (what CI tests of dmabufs)
- Date: 2026-11-12

## Context

[ADR-0025](0025-applications-in-ci.md) checks that each application in CI shows its window, goes
quiet, and leaves Yuna at rest for 10 s. Measured on 2026-11-12, in `yuna --headless` on
llvmpipe, at 1920 × 1080 and 60 Hz:

- Alacritty 0.17, GTK 4.24's `gtk4-demo` and Qt 6.12's `qml` on a still file draw 2 or 3 frames,
  then nothing: Yuna then has no context switch in 10 s.
- `vkcube` turns its cube without end. It stops only after `--c <frames>`, or paused by the space
  key, and Yuna gives no input before M1.5. Left to draw, Yuna draws 60 frames a second for it.
- SlimeEngine's `window` example draws without end too: it runs its frames continuously unless
  its program asks for frames on demand, which the example does not. It stops after
  `--frames <count>`, and has no pause.
- All five draw through `wl_shm`, none through `linux-dmabuf`, although Yuna offers it: Mesa's
  EGL and Vulkan on the CPU (llvmpipe, lavapipe) fall back on shared memory when the compositor
  names no GPU. GTK draws through Vulkan on lavapipe, Qt and Alacritty through EGL on llvmpipe.

## Decision

- **`vkcube` and SlimeEngine's `window` run in CI from M1.4 on**, each drawing 120 frames and
  then leaving: Yuna must get a commit for each, draw them, and rest 10 s once its client left.
- **Their rest with their window open waits for M1.5**, which brings input: the space key pauses
  `vkcube`. SlimeEngine's example has no pause; M1.5 finds how it rests, or says why it cannot.
- **`vkcube` left to draw is the proof** that a client that keeps drawing fails the rest check.
- **ADR-0025's second job is a workflow of its own**, `.github/workflows/slimeengine.yml`: its
  events, weekly, on demand and on a change of the drawing path, are not CI's.

The owner chose the first two on 2026-11-12.

## Alternatives considered

- **A number of frames as their whole check**, their rest after they leave counting as done: M1.4
  done at once, but a Vulkan client that rests with its window open would never be checked.
- **Asking SlimeEngine for an on-demand option in its example**: SlimeEngine's rest with its
  window open checked sooner, at the cost of a change in another project, outside M1.4.

## Consequences

- M1.4's check of rest covers Alacritty, `gtk4-demo` and `qml` with their windows open; `vkcube`
  and SlimeEngine's example only after they leave. M1.5's done-when gains their rest.
- ADR-0022's consequence that CI does not test the dmabuf path no longer holds whole. Since M1.4's
  B2, the compositor's in-process tests import a client's dmabuf on llvmpipe and draw it, from a
  sealed memfd. No real application in CI uses `linux-dmabuf`, and a GPU's dmabufs are tested
  nowhere ([debt](../debt.md)).
- Yuna's refusal of a dmabuf whose file may shrink touches none of these applications.
