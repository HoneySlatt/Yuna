# 0043. Text input on an agent's seat, and the rest of SlimeEngine's examples

- Status: Accepted; amends [0037](0037-seats-tools-input-and-closing.md) (how `type` types a
  character outside `us`) and [0029](0029-the-rest-of-animated-applications.md) (which
  SlimeEngine client's rest M1.5 checks)
- Date: 2026-11-13

## Context

M1.5 ends with `text-input-v3` and the rest of `vkcube` and SlimeEngine's `window` example with
their windows open ([PLAN](../PLAN.md#m15--agents-control)). Facts, read and run on 2026-11-13:

- `text-input-v3` connects a client's text field to an input method. Smithay `8fd9fe0` passes
  a text field's requests to an input method client (`input-method-v2`) and drops them when
  none runs, unless the compositor acts as the seat's input method
  (`TextInputHandle::set_compositor_input_method`). It then sends `enter` to the focused
  client's text inputs, which follow the keyboard's focus. The compositor sends them text
  (`commit_string`) and text being composed (`preedit_string`). No input method runs in
  `yuna mcp`.
- `type` types a character outside the `us` layout through Smithay's `inject_text_keysyms`
  ([ADR-0037](0037-seats-tools-input-and-closing.md)). It gives the client a keymap of its own
  for the run of characters, then the seat's back. A client that handles a keymap change badly
  types wrong characters.
- SlimeEngine's text fields (uwui) take text typed, and text composed through an input method;
  winit asks for `text-input-v3` while such a field holds the focus.
- SlimeEngine's `window` example asks a frame of its own at its frame rate for as long as its
  window shows (`AppConfig::on_demand` is off), and has no pause. Hidden, it draws when Yuna's
  frame callbacks come, once a second (`frames.rs`). Its `editor` example runs its frames on
  demand. In `yuna mcp`, on llvmpipe at 1920 × 1080 and 60 Hz, the editor opened on its yard
  left Yuna with no frame and no context switch for 10 s, its window shown. `vkcube`, paused by
  its space key through `keyboard`, did the same.

## Decision

The owner chose, on 2026-11-13:

- **Yuna is the input method of every agent's seat.** Each agent's seat is made with Smithay's
  compositor input method on, so that a client's text field is entered and may be enabled. The
  owner's seat has none: a real input method comes with the desktop.
- **`type` keeps typing keys.** A character of the `us` layout is typed by its key. A run of
  characters outside it goes to the focused text field as `commit_string` when that field has
  enabled `text-input-v3`, and through `inject_text_keysyms` otherwise.
- **`keyboard preedit`** shows text being composed in the focused text field, its cursor at its
  end; empty text clears it. It is refused when no text field is enabled. The next `type`
  replaces it, as an input method's commit does.
- **`keyboard` tells whether a text field is enabled** where its keyboard is, in each result.
- **SlimeEngine's `window` cannot rest with its window shown**, by its design: it draws at its
  frame rate while it shows. It stays checked after it leaves (ADR-0029). **SlimeEngine's
  `editor` is checked at rest instead**, its window shown: a SlimeEngine client on wgpu and
  Vulkan that rests. Nothing changes in SlimeEngine.

## Alternatives considered

- **`text-input-v3` offered, with no input method**: it follows the focus and does nothing
  until a real input method is plugged in, at the desktop.
- **No `preedit`**: smaller, but an agent could not test how an application shows text being
  composed.
- **Everything `type` types sent as `commit_string`**: no key would reach the application, so
  its shortcuts and its key handling would go untested.
- **An option `--on-demand` in SlimeEngine's `window` example**: `window` itself checked at
  rest, at the cost of a change in another project, where a session is working.
- **The reason alone for `window`**: no SlimeEngine client would be checked at rest with its
  window shown.

## Consequences

- An agent types any character into a text field that speaks `text-input-v3` with no keymap
  change; outside one, as before.
- Every client of an agent's seat that binds `text-input-v3` gets `enter` when its surface has
  the keyboard, as under any input method.
- The done-when's story runs SlimeEngine's `interface` example, whose bar holds buttons and a
  text field. It and the editor's rest need SlimeEngine's cooked data: they run in the weekly
  SlimeEngine job, not in CI's.
- `vkcube`'s rest needs input: it runs through `yuna mcp`, in CI.
