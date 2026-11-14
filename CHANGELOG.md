# Changelog

What changed for users, newest first. Yuna has no release yet: everything below comes with 0.1
([ROADMAP](docs/ROADMAP.md)).

## Unreleased

- A virtual output may run under VRR, as amdgpu drives a DisplayPort Adaptive-Sync monitor:
  each frame shown when it is ready, within the range, and below it shown several times as the
  driver does; clients see it in their presentation times. It may also announce the HDR colours
  of the owner's monitor, a PG32UCDM. `--output` takes `vrr=MIN` and `hdr`, as in
  `3840x2160@240,1.5,vrr=48,hdr`; `yuna mcp`'s `seat create` and `outputs create` take `vrr_min`
  and `hdr`, and `outputs list` tells each output's VRR range and monitor. `windows` with
  `colour: true` gives the colours each window's surfaces declared.
- Clients can describe their surfaces' colours through `color-management-v1`, and read their
  outputs': an HDR output announces the owner's monitor's colours, an SDR one sRGB. Nothing is
  drawn differently yet.
- A client sees only its own seat and that seat's outputs: an agent's programs no longer see
  another seat's `wl_seat` or `wl_output`.

- `yuna mcp` is the input method of each agent's seat: `keyboard type` commits a character
  outside the `us` layout to a text field that speaks `text-input-v3`, with no keymap change;
  `keyboard preedit` shows text being composed there; `keyboard get` and every `keyboard` result
  tell whether a text field is enabled where the keyboard is.

- `yuna mcp`'s `capture` gives a PNG of a seat's output, a region of it or one of its windows,
  at the logical size by default, where a pixel is a unit `pointer` takes, or at the physical
  size. Its result tells how the image maps to the output and gives an id, through which
  `pointer` aims at a pixel of that image.
- Clients can capture their seat's outputs through `ext-image-copy-capture-v1`, as grim does.

- `yuna mcp` serves several agents, each on a seat of its own: `seat` creates one, with an
  output and a Wayland socket its programs use, lists them and closes one, ending its programs.
  `pointer` moves, clicks, drags and scrolls, `keyboard` presses keys, taps combinations and
  types any text; `workspaces` lists, creates, shows and fills workspaces. Every tool but `clock`
  names its seat, and no input, focus or clipboard of one seat reaches another. `yuna mcp` no longer starts an output
  of its own; its `--output` is each new seat's first output.
- Clients get `cursor-shape` and drag and drop.

- `yuna mcp` is an MCP server for one agent, on its standard input and output: a headless Yuna
  of its own, which speaks MCP's revisions 2026-07-28 and 2025-11-25. Its tools create, list and
  remove virtual outputs (`outputs`); start, list and stop programs, each with a clean
  environment and a log (`spawn`); list windows, waiting for one if asked (`windows`); and read
  the clock, or step it with `--manual-clock` (`clock`). Closing its input stops it and ends
  every program it started.

- `yuna --headless` runs Yuna without a screen, on virtual outputs drawn offscreen, and prints
  `WAYLAND_DISPLAY=<socket>`; SIGTERM or Ctrl-C stops it and removes its socket.
- `--output WIDTHxHEIGHT@RATE,SCALE` adds a virtual output; the default is one output of
  3840 × 2160 at 240 Hz and scale 1.5.
- Yuna draws on Mesa's llvmpipe by default; `--gpu` draws on the GPU's render node instead.
- `YUNA_LOG` sets the level of its logs: `error`, `warn`, `info`, `debug` or `trace`.
- Clients get `xdg-shell` windows placed in scrolling columns, with server-side decorations and
  no title bar; popups, `xdg-activation`, `xdg-dialog`, `wl_shm` and `linux-dmabuf` buffers,
  viewporter and fractional scale, presentation time, a keyboard, the clipboard, the primary
  selection and `ext-data-control`.
- Alacritty, GTK 4, Qt 6 and Vulkan applications run in it, and it draws nothing while they rest.
- A frame is drawn just ahead of its output's refresh, or at once after a quiet moment, and is
  presented at the boundary it aimed at when drawn in time: a commit shows about one period after
  it, no longer two. Presentation feedback arrives at the boundary itself, as from a monitor.
