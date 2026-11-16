# Changelog

What changed for users, newest first. Yuna has no release yet: everything below comes with 0.1
([ROADMAP](docs/ROADMAP.md)). A change that users see adds a fragment to
[`changelog.d/`](changelog.d/README.md); `scripts/changelog.py --merge` brings them here at each
release.

## Unreleased

- X11 programs can connect: each seat with a Wayland socket gets an X display of its own, and
  the first X11 program to connect starts Xwayland for it, found on `PATH`, behind the seat's
  own cookie; Xwayland stops 10 s after its last X11 program leaves, and a crash of it leaves
  Yuna running. `yuna --headless` prints `DISPLAY=:<n>` and `XAUTHORITY=<file>` after
  `WAYLAND_DISPLAY`; `yuna mcp`'s programs get their seat's. X11 programs read `Xft.dpi` and
  XSETTINGS of the seat's largest output scale. On llvmpipe, the default, Xwayland draws on the
  CPU and offers no GLX.
- X11 programs' windows open in the layout as Wayland programs' do: tiled, focused, typed into
  and clicked, made fullscreen or full width when they ask, hidden and told so, closed through
  `WM_DELETE_WINDOW` or by ending their program, sliding with the motion. They are drawn sharp
  at the seat's largest scale and the right size on every output, averaged where an output's
  scale is smaller. A fullscreen game that sets an output's mode through RandR keeps it, drawn
  pixel for pixel. `yuna mcp`'s `windows` gives each window's `shell`, `xdg` or `x11`, and an
  X11 window's class as its application id. Java programs need `_JAVA_AWT_WM_NONREPARENTING=1`.
  Wine reads its prefix's `LogPixels`, not `Xft.dpi`: a Wine program is the right size at 1.5
  once its prefix's DPI is 144 (96 × the scale); X11 programs that do not scale themselves, such
  as xterm, are small on outputs at that scale.
- X11 programs' menus and tooltips show where the program puts them, over their window and
  sliding with it, or alone over every window; a click reaches the pixel under the pointer, and
  the keyboard stays with the window. Dialogs open floating at their own size, over their
  window. A resized X11 window that takes `_NET_WM_SYNC_REQUEST` shows its new size whole, held
  100 ms at most. A window asked to close that leaves its ping unanswered 5 s is marked
  `unresponsive` in `yuna mcp`'s `windows`, never ended for it. A program started with an
  activation token in its startup id is activated with it. A window mapped again starts without
  the states it had. Huge or malformed properties, nonsense sizes, messages naming unknown atoms,
  floods of thousands of windows and programs dying mid-request leave Yuna running, another
  window's frames on time.

- Animations, off by default in `yuna --headless` and `yuna mcp`, where each change still shows
  at once; `--animations` turns them on in both. The view slides along the strip of columns,
  windows the layout moves slide to their new places, workspaces slide past each other when
  switched, and the special workspace fades in and out; fullscreen stays instant. Each frame
  shows the motion at the time it is presented, and the last shows the windows exactly at their
  places; then Yuna draws nothing more. Clients are told they are shown or hidden from their
  places, at most once per change, never at each frame. Clicks, the pointer and window captures
  follow where windows are drawn; `yuna mcp`'s `windows` gives each window's drawn `rect`, its
  `place`, and `motion_ends_ns`, when its output's motion ends. `--reduced-motion` turns them
  on under reduced motion: nothing slides, and the special workspace's fade is kept, shorter.

- A virtual output may run under VRR, as amdgpu drives the owner's monitor, checked against two
  traces of it: each frame shown a vertical blank after it is ready, within the range, and below
  it shown several times as the driver does; clients see it in their presentation times. It may also announce the HDR colours
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
