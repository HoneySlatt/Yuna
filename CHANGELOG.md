# Changelog

What changed for users, newest first. A change that users see adds a fragment to
[`changelog.d/`](changelog.d/README.md); `scripts/changelog.py --merge` brings them here at each
release.

## [Unreleased]

## [0.1.0] - 2026-11-19

Stage 1, the headless core ([ROADMAP](docs/ROADMAP.md), [history](docs/history.md)): Yuna runs
without a screen, for agents and tests.

### Added

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
- `yuna mcp`'s `accessibility` tool reads the accessibility trees of a seat's programs started
  with `spawn start` and `accessibility: true`, and finds a node by its name, role and states: its
  window, process, description and rectangle on its output, in `pointer`'s units, so that an agent
  clicks a button it found by name. Each agent's seat has an accessibility bus of its own,
  `dbus-daemon`, started with its first such program and ended with the seat, on which Yuna is
  the AT-SPI registry: the one `YUNA_DBUS_DAEMON` names, nixpkgs' own in Yuna's package, else
  the first on `PATH`. Tested with GTK 4, and Qt on Wayland and on X11; Chromium and Electron
  also need `--force-renderer-accessibility` for a page's content.
- `yuna --headless --agent-seat` serves an agent's seat on its socket rather than the owner's: a
  `us` keyboard, a pointer, and the virtual keyboard and pointer, `zwp_virtual_keyboard_v1` and
  `zwlr_virtual_pointer_v1`, which drive its programs as in a headless sway. They are offered on
  agents' seats alone, in `yuna mcp` too, never on the owner's.
- `keyboard layout` changes an agent's seat's keyboard layout while Yuna runs: XKB's layouts,
  their variants and its options, on the evdev rules and a pc105 keyboard, `us` at first. Every
  program of the seat gets the new keymap, X11 programs included, and `type`, `tap` and `press`
  go through it: `type` types each character the layout has with its own keys, AltGr held for
  level 3, and locks another of the layout's groups with the layout's own switch, such as
  `grp:alt_shift_toggle`'s Alt and Shift, then the group it started in again. A layout, variant
  or option XKB's rules do not know is refused, and so is a change while a key is held, with
  nothing changed. `keyboard get` tells the seat's layout, variant and options.
- `yuna mcp`'s `clipboard` reads, sets and clears a seat's clipboard or primary selection, as its
  X11 and Wayland programs copy and paste them (ADR-0033, ADR-0091). `get` gives the MIME types
  offered and the data of one, by default the first text type, as text when it is UTF-8, else in
  base64; 64 KiB by default and 1 MiB at most, `truncated` past them; it waits 5 s at most for
  the program that copied, then answers with what came and `timed_out`. `set` makes Yuna hold
  up to 256 KiB of text, offered as `text/plain;charset=utf-8`, `text/plain` and `UTF8_STRING`,
  until a program copies another or `clear`, which empties the selection whoever copied it.
  Yuna writes it to each program that pastes, at its pace, and stops after 5 s without
  progress.
- `yuna mcp`'s `events` tool: an agent waits for what happens on its seat rather than asking in a
  loop, or reads what happened since a cursor it keeps. Windows opened, closed, retitled, placed
  and unresponsive, the seat's focus, programs that exited, Wayland connections that ended, with
  the protocol error Yuna sent and the program they belonged to, and outputs added, removed and
  changed; each seat keeps its last 1,024 events, and a reply tells how many a cursor too old
  missed (ADR-0033).
- A Nix package, `packages.x86_64-linux.yuna`, also the flake's default, built in CI: `yuna`
  draws with the package's own Mesa and finds Xwayland, so that it runs the same on NixOS, on
  another distribution with Nix and in a CI, at about 1.4 GB to fetch. The programs `yuna mcp`
  starts get the caller's own `PATH` and libraries, not the package's.
- `yuna mcp`'s `outputs configure` gives an existing output another size, rate, scale, VRR
  range or HDR, each value left out its own, whole or not at all, as a monitor set to another
  mode; `vrr_min: 0` turns VRR off. Its clients are told at once, its windows are placed again
  in it, the seat's outputs on its right move with its right edge, and its refreshes start
  again from the change. A program that keeps its size, as a fullscreen game does, is placed
  by it again once it answers the new mode. X11 programs keep the scale their Xwayland started
  with. It answers with the output as `outputs list` gives it.
- The rest auditor names what woke Yuna and what made it draw: each source of its loop, each
  client by its process, seat and, in `yuna mcp`, program, and each frame by its causes. MCP's
  `rest` gives the counts since Yuna started, or those of the next `wait_ms`, its own timer left
  out; `--rest-audit`, for `yuna --headless` and `yuna mcp`, logs one line for each second in
  which something woke Yuna, and nothing at rest (ADR-0062).
- `spawn log` reads a program's log through MCP, by byte offsets since it started or its newest
  bytes; `spawn list` gives each program's last protocol error and how much of the time it may
  run.
- `spawn freeze`, `thaw` and `slow` make a program stuck or slow: its process group stopped,
  continued, or run a share of each 100 ms, on real time. Every path that ends programs with
  SIGTERM sends SIGCONT after it, so that a stopped program ends at once rather than at SIGKILL.
- `wlr-screencopy-unstable-v1`, version 3, beside `ext-image-copy-capture-v1`: tools that copy an
  output or a region of it through the older protocol, uwui's measures among them, work in
  Yuna. A client copies its own seat's outputs into `wl_shm` buffers; a copy with damage waits
  for something new without waking Yuna at rest.
- The clipboard and the primary selection pass between a seat's X11 programs and its Wayland
  programs, both ways, through Yuna's X11 window manager (ADR-0066): text as `UTF8_STRING`,
  `STRING` and `TEXT`, any MIME type as it is named, large ones through INCR. An X11 program
  reads the Wayland programs' selections only while one of the seat's X11 windows has the
  keyboard; a copy made before the seat's Xwayland started is pasted in X11 too. A transfer that
  stalls 5 s ends. Drag and drop between them comes later.
- Drag and drop between a seat's X11 and Wayland programs, both ways: text, files and any type,
  in XDND version 5 with X11 targets of versions 3 to 5, through each seat's X11 manager; an
  agent drags through MCP's pointer as a user does (ADR-0102).
- The rest auditor names the drags' time limits as a wake source of their own, `clock_drags`
  (ADR-0096).

### Changed

- `clipboard get` gives in base64, with `base64` true, data holding a control character other
  than tab, newline and carriage return, though it is UTF-8: as text, each took 13 bytes of the
  reply.
- On llvmpipe, `linux-dmabuf` is version 6, with feedback whose main device is no device
  (`dev_t` 0), rather than version 3: a Mesa Vulkan program on a GPU, GTK 4's among them, then
  shares linear copies of its frames outside its video memory, which llvmpipe reads, and its
  windows show (ADR-0098). It works for programs that compare that device with their GPU as
  Mesa does; Chromium, Firefox, VA-API's video decoders and NVIDIA's driver are not verified.
- On llvmpipe, `linux-dmabuf` offers only the formats of one plane whose pixel size Yuna knows,
  23 with Mesa 26.2 (red, red and green, and RGB), each linear or with no modifier given, rather
  than all 57 llvmpipe imports: its YUV formats, packed or of several planes, are no longer
  offered there.
- `yuna --help` names every variable a user sets for Yuna, `YUNA_DBUS_DAEMON` now among them,
  and lists its exit statuses: 0 for success, 1 for a failure, 2 for a usage error and 101 for a
  panic of its state (ADR-0079).
- `yuna mcp` refuses an output whose side passes 8,192 pixels, and one that would bring all its
  outputs past the pixels of 16 outputs at 3840 × 2160, whether `seat create`, `outputs create`
  or `outputs configure` asks for it: before, 16 outputs at the renderer's limit could take
  16 GiB. The tools' schemas give the side's maximum.
- A program's log in `yuna mcp` keeps the first 256 KiB of its output and its newest 1 MiB,
  rather than its first 1 MiB: both a failure to start and the end show. `log_bytes` and
  `log_dropped` count what it keeps and what it dropped between.
- A program `yuna mcp` starts through `spawn` no longer runs on the user's home: each seat's
  programs share a throwaway home in `TMPDIR`, or `/tmp`, which `HOME` names, with
  `XDG_CONFIG_HOME`, `XDG_DATA_HOME`, `XDG_CACHE_HOME` and `XDG_STATE_HOME` inside it; it goes
  when the seat closes and when Yuna stops, and one a killed Yuna left goes when the next
  `yuna mcp` starts. Their folder is made again when a cleaner of the temporary folder removes
  it while Yuna runs, and under a random name when another user took its own. `XDG_CONFIG_DIRS`
  and `XDG_DATA_DIRS` are kept. Name any of these variables in `spawn start`'s `env` to give a
  program a home of your choosing (ADR-0105).
- A program `yuna mcp` starts without `accessibility: true` now gets `NO_AT_BRIDGE=1` and
  `GTK_A11Y=none`, so that its toolkit never looks for an accessibility bus; give either in
  `spawn start`'s `env` to choose otherwise.
- `type` reads the keyboard's state as its programs read it: with Caps Lock on, or Shift held, a
  letter still types as written. It types with a PC keyboard's keys first, `(` and `)` with Shift
  and 9 or 0 rather than the keypad's parentheses, and never with a key past 255, which X11
  programs do not get. A tap or a press of `parenleft` still presses a key that types `(` alone,
  and a release lets go of the key held under that name even after another group is locked.
- `yuna mcp`'s `workspaces` refuses a workspace's name past 256 bytes, in `create`, `remove`,
  `focus` and `move_window`, and its schema gives the bound: `windows` repeats the name for each
  window, and a name bounded by a request's line alone made one reply gigabytes.
- An X11 window's title and class are kept to 4 KiB, cut at a character's edge, where 64 KiB
  were; MCP's `windows` gives them so. An X11 program's windows past its thousandth, mapped or
  not, are followed without their properties and never show, as windows past a program's limit
  in the layout: one that asks to be mapped lies below every X11 window Yuna stacks, so that it
  takes no click from one Yuna shows, and stays out of `_NET_CLIENT_LIST`; an override-redirect
  one, which maps itself, is not lowered (ADR-0131).
- Xwayland, and the `xkbcomp` it runs, get only the variables of Yuna's environment that they
  and their drivers read (the path, the home, the locale, Mesa's, Vulkan's and EGL's), and their
  Wayland connection: nothing else of the program that started Yuna, an agent's harness in
  `yuna mcp` included.

### Fixed

- One client making Yuna hold descriptors until its table is full, `wl_shm` pools for instance,
  no longer stops `yuna --headless` at the next connection: the owner's socket pauses and takes
  the connection once descriptors are free, as an agent's seat's socket did already. Yuna also
  raises its soft limit of open files to the hard limit at start, as KWin and Mutter do, and
  gives the programs it starts, Xwayland among them, the limit it started with. A bound on the
  descriptors one client may make Yuna hold is still to come.
- A panic of Yuna's state stops it at once, with status 101, rather than unwind through a state
  left half changed; panics on its other threads are handled as before. `yuna mcp` first ends
  every process under it, SIGTERM then SIGKILL 2 s later, 3 s at most, and removes its folder in
  `XDG_RUNTIME_DIR`, which it used to leave behind (ADR-0080).
  Its X displays' files in `/tmp` go too; `yuna --headless` leaves its Wayland socket and its
  lock, which the next start takes as stale.
- An X11 window that the layout, full or refusing it, refuses once its surface has committed,
  when Yuna reads its serial late, gets its frame callbacks at the hidden windows' rate from
  then on, rather than never: Xwayland, which waits for them, keeps drawing it.
- A virtual keyboard holding more than about 1,018 keys no longer gets the clients of its seat
  ended, whose `wl_keyboard.enter` grew past a Wayland message: a seat holds 64 keys and 16
  pointer buttons at most. A virtual keyboard's or pointer's press past them, or of a key its
  keymap does not have, is dropped; MCP's `keyboard` and `pointer` presses past them are refused
  (ADR-0028).
- An X11 program that sends a made-up `KeymapNotify` to the root no longer stalls its seat's X11
  window manager: the seat's next windows open, and its clipboard and drags go on.
- An X11 window manager that stops taking Yuna's commands, its Xwayland stopped or hung, no longer
  makes Yuna's memory grow: what Yuna owes it is bounded, and once it has taken none for 10 s,
  and still none 1 s later, its Xwayland ends, as when the manager fails, and the seat's next X11
  program starts a new one. Its watchdog is disarmed as soon as the manager takes what it owes:
  nothing is left armed at rest, and a manager that takes them within a second of Yuna running
  again, after Yuna itself was stopped, is not taken for stalled.
- A program on a seat's accessibility bus can no longer end that bus for the whole seat. Yuna's
  registry records an application as the connection that embeds it, and refuses with
  `InvalidArgs` an `Embed`, `Unembed` or `DeregisterApplication` that names another; Yuna sends
  no message whose names or path the bus would disconnect it for, so that a node a toolkit
  places at the bus's reserved path, `/org/freedesktop/DBus/Local`, goes unread alone
  (ADR-0094). The bus used to disconnect Yuna, which then ended the bus: every program of the
  seat lost its accessibility tree for the rest of its life.
- Yuna no longer spins at full processor once a seat's accessibility bus has ended, when its
  connection to that bus could be watched only in part, as when file descriptors run out.
- The activation tokens of clients that have left are bounded, 256 together, the oldest
  forgotten first: a client that connects again and again to ask tokens no longer grows Yuna's
  memory, and a launcher's token still outlives its launcher (ADR-0131).
- One commit of a window reads its tree of subsurfaces once, rather than once for each of its
  surfaces: a window that commits 4,000 subsurfaces side by side holds Yuna about a fifteenth as
  long.
- Windows refused by the caps on windows no longer lengthen every arrangement, and a new window's
  first configure no longer copies its seat's layout: on a seat of 4,000 windows, it takes
  microseconds rather than milliseconds.
- A client's capture that cannot be read logs a warning once, then at debug level until a refresh
  reads every client's capture of the same output again, rather than a warning at each refresh.
- `yuna mcp` never encodes more than four captures at once, however many seats capture together:
  a capture copied while four are encoded waits for one of them, and is answered after it.
- On llvmpipe, the default, a program drawing with the machine's GPU no longer kills Yuna. Yuna
  refuses a dmabuf whose file llvmpipe cannot map, as a GPU's buffer in its video memory, and
  one whose stride is shorter than a row of its pixels: the program is told `failed`, or ended
  by `invalid_wl_buffer` for `create_immed`, and every other program goes on (ADR-0080,
  ADR-0098). Mesa's llvmpipe took such buffers, then read address zero, or past the buffer's
  end, when it drew them. A GPU program that is not Mesa's, Chromium for one, may still send
  such a buffer, and is then ended alone; which ones do is not verified.
- `yuna mcp` holds the answers of calls that waited while 16 MiB of replies wait for a client
  that does not read them, as it holds new requests: 64 calls answered at once no longer queue
  past the bound. An `accessibility` walk keeps a name, a description or a toolkit's name cut as
  it gives them, 1,000 characters, and leaves out an object path past 4 KiB, whatever a program
  on the seat's bus sends.
- A `yuna mcp` that starts removes the folders of sockets and logs that one killed by SIGKILL
  left in `XDG_RUNTIME_DIR`: each live Yuna holds a lock on a file in its folder, which the
  kernel releases at any end, and a folder whose lock is free is removed, never by its pid.
- `seat close` ends the seat's own running programs, and the processes under them, whatever
  `YUNA_SEAT` their environment names, another seat's included, and a process left in the group
  of one of its programs whose `YUNA_SEAT` names a seat that never existed: before, both outlived
  the seat, out of `spawn`'s reach.
- A window no longer jumps on screen when a workspace leaves, or is shown, during a switch between
  workspaces; a window of a workspace that leaves a switch now vanishes at once (ADR-0099).
- An X11 program's drag moved onto a Wayland window and released in the same instant, as MCP's
  `release` given a place does, drops into an X11 window hidden or covered under that point far
  less often: Yuna keeps the window it lays over X11 for the drag after the release, until the
  source drops or leaves, the pointer moves again, or five seconds run out. A program that looks
  for its target before Yuna has laid that window still finds the X11 window under it (Known
  limitations; ADR-0102).
- One Xwayland shows at most 256 menus and tooltips at once, all its X11 connections together:
  one program opening many connections no longer passes the 64 of each. A menu or tooltip past
  the 256 waits, and shows once another of its Xwayland goes, lowest in X11's stacking first: a program
  that floods its Xwayland over several connections delays another's tooltip until its own menus
  go, no longer for good.
- An X11 window whose size hints ask a minimum larger than every output of its seat, up to
  X11's 32,767 pixels, is given no more than the seat's largest output on each axis, and opens,
  rather than being configured to a size nothing shows. Its minimum follows the seat's outputs as
  they are added, removed or given another mode; a seat without an output bounds it to nothing
  until it has one.
- In `yuna mcp`, an Xwayland that crashes once its window manager is in charge counts as a crash
  whichever of Yuna's reapers takes its status, so that a crash loop pauses its X display as in
  `yuna --headless`, rather than start Xwayland again at each X11 connection.

### Security

- An activation token is used only by a window of the seat it was asked on: a window of another
  seat that names it, through xdg-activation or an X11 window's startup id, is treated as having
  no valid token and leaves it unused, so that it can no longer stop the launched program from
  taking the focus (ADR-0026).

### Known limitations

- After an X11 program's drag released onto a Wayland window, the window Yuna lays over X11 for
  the drag stays until the pointer comes over an X11 window, or the drag ends; a click on an X11
  window given with that motion, as MCP's `click` given a place does, can reach Yuna's window
  rather than the X11 window, and be lost, when it reaches Xwayland before Yuna's X11 window
  manager, on its own thread, has taken that window away (ADR-0102).
- An X11 program's drag moved onto a Wayland window and released in the same instant can still
  drop into an X11 window hidden or covered under that point, when the program looks for its
  target before Yuna's X11 window manager, on its own thread, has laid its window over X11 for
  the drag: about 1 time in 12 on a quiet machine with Yuna's test program, which looks within a
  millisecond; a toolkit that looks sooner may lose more often. Such a drop is refused, never
  given to the Wayland window. Moving onto the Wayland window, then releasing in a later call,
  avoids it (ADR-0102).
