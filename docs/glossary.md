# Glossary

Terms Yuna uses in a sense of its own, or that a newcomer may not know.

- **Achieved rate** (bench): the frames a second a compositor really drew in a cost window: its
  own exact count, else in `pacing` the frames it presented, else the frames window's capture, a
  lower bound. On wlroots' headless timer, a frame 4 ms after each commit, it is up to 250 for a
  240 Hz mode; at the mode's rate, 240 at most. The run's file keeps it; it decides no
  ranking: the *pace* does.
- **Activation token**: a string a client asks Yuna for and hands on, a launcher to the program
  it starts, so that a window can be activated with it (`xdg-activation-v1`); valid only when
  asked from the surface that has the keyboard focus, a window or its open menu, and for 10 s.
- **Agent's output**: a virtual output given to one agent, never shown unless the owner asks, drawn
  at full rate ([ADR-0015](adr/0015-agents-workspaces.md)).
- **Area filter**: how Yuna draws an X11 window on an output below XWayland's scale: each
  physical pixel the average of the X11 pixels it covers, weighted by how much of each, in a
  texture shader of Yuna's ([ADR-0068](adr/0068-x11-windows-drawn-by-yuna-through-an-area-filter.md)).
- **As shipped**: a rival run with its own defaults in the rivals' bench, beside *equal work*
  ([ADR-0075](adr/0075-the-rivals-measured-headless-now-on-a-real-display-at-stage-2s-exit.md)).
- **Asked size**: the size a window's client is told in a configure, in whole logical units; 0 on
  an axis lets the client choose it ([ADR-0026](adr/0026-real-clients-in-the-layout.md)).
- **Bench shell**: `nix develop .#bench`, the development shell with the compositors and clients
  the rivals' bench runs, which CI's shell leaves out.
- **BTR**: below-the-range, amdgpu's frame insertion: under a VRR range's least rate, each frame
  is shown several times, in refreshes of a fixed length, to keep the panel inside its range
  ([ADR-0046](adr/0046-emulated-vrr-amdgpus-rule-checked-on-the-monitor.md)).
- **Camera**: the position of a scrolling workspace's view on its strip, `view_x`; it moves on a
  spring while windows keep their places ([ADR-0052](adr/0052-motion-in-a-pure-crate.md)).
- **Capture mapping**: how a capture's image maps to its output's logical coordinates: the
  logical point of its top-left corner, and the logical units one pixel covers.
- **Column**: in scrolling columns, windows stacked from top to bottom that are asked one width;
  the column is as wide as the widest of them as drawn.
- **Committed size**: the window geometry a client last committed; the layout draws the window at
  it, and places scrolling columns by it ([ADR-0026](adr/0026-real-clients-in-the-layout.md)).
- **Core** (bench): a compositor's own process and its descendants, in the `core` control group of
  its sandbox; its **stack** is every process of the sandbox, its session bus included, the
  bench's clients left out.
- **Cost window** (bench): a measured window whose costs the bench reads; for a compositor whose
  frames are counted by capture, the **frames window** after it runs the same scenario again with
  the frame counter, whose copies would weigh on the costs.
- **Cursor** (MCP): the number an agent gives `events` as `after`, an event's `seq` or a reply's
  `next`: the agent keeps it, Yuna keeps nothing between calls.
- **Data control**: `ext-data-control-v1`, which lets a client, such as a clipboard manager, read
  and set the selections without the keyboard focus.
- **Development shell**: the environment `nix develop` opens from `flake.nix`, with the pinned
  toolchain and every tool the checks use; CI runs in it too.
- **Dialog**: a window that names a parent, as xdg-shell's `set_parent`; it opens floating, centred
  on its parent, and takes the focus only from it.
- **Dismissed popup**: a popup Yuna told `popup_done`, as xdg-shell lets a compositor do: it
  leaves its window, is no longer drawn, takes no keyboard grab, and waits for its client to
  destroy it.
- **Disturbed** (bench): said of a measured window during which the machine's other programs
  used more than 2 cores, or more than 10 % of the GPU's drawing, on average; a run with one is
  shown, marked †, never ranked.
- **Dmabuf**: a buffer a client shares with Yuna by file descriptor, through `linux-dmabuf`,
  most often in GPU memory; Yuna offers the protocol only where its renderer imports dmabufs.
- **Drawn rectangle**: where a frame draws a window during motion, rounded once to the physical
  grid; pointer, clicks and captures use it. Its client is told from its **place**, the
  rectangle the layout gives it ([ADR-0054](adr/0054-motion-sampled-at-presentation-drawn-apart-from-told.md)).
- **DRM usage statistics**: what the kernel counts for each client of a GPU, read in the
  `fdinfo` of its render node: each engine's busy time in nanoseconds (`drm-engine-gfx`), the
  measure of a frame's GPU time ([ADR-0030](adr/0030-a-frames-gpu-time-from-the-kernel.md)).
- **Dwindle**: a layout where each new window splits the space of the focused one in two.
- **Dynamic workspaces**: an option of the layout: each output keeps one empty unnamed workspace
  last, as niri does ([ADR-0018](adr/0018-workspaces.md)).
- **EDID**: what a monitor tells the computer of itself: its modes, its VRR range, its colours
  and HDR luminances; the owner's monitor's give the HDR outputs' colours.
- **Equal work**: a rival run with its effects, wallpaper and XWayland off in the rivals' bench, so
  that compositors that draw more are compared on the same work
  ([ADR-0072](adr/0072-budgets-set-against-the-peers-the-same-day.md)).
- **Fake fullscreen**: an application told it is fullscreen while its column keeps its size.
- **Fatal path**: what Yuna does when its state thread panics: it stops at once, never unwinding
  through a state left half changed; `yuna mcp` first ends its programs and removes its folder
  ([ADR-0080](adr/0080-what-survives-a-failure-and-yunas-own-panics.md)).
- **Floor** (bench): the figures of the rivals that draw least, sway and dwl, recorded beside
  the budgets, never a budget ([ADR-0072](adr/0072-budgets-set-against-the-peers-the-same-day.md)).
- **Fractional scale**: an output scale that is not a whole number, such as 1.5. A client that
  follows `wp_fractional_scale_v1` draws its buffer at its output's physical size and sets its
  viewport (`wp_viewporter`) to its logical size: one buffer pixel per physical pixel.
- **Full width**: a column that spans the working area, gaps aside; in dwindle, a window
  maximised over the working area. Umbriel calls it "maximize".
- **Fullscreen state**: what Yuna tells the shell is fullscreen: a game, a film, plain fullscreen
  or nothing; the shell decides what follows, such as holding notifications back
  ([ADR-0057](adr/0057-what-yuna-does-and-what-the-shell-does.md)).
- **Game mode**: the state Yuna enters when a game is fullscreen: effects and animations off,
  notifications silent, VRR in its band.
- **The games' band**: the high, narrow range of refresh rates Yuna keeps a game's VRR in, by
  showing frames several times ([ADR-0014](adr/0014-vrr-without-flicker.md)).
- **Greeter mode**: Yuna running under greetd as the login screen's compositor, without command
  socket, MCP, global shortcuts, portals or XWayland ([ADR-0065](adr/0065-what-hollow-asks-of-yuna.md)).
- **HDR10**: high dynamic range output as monitors take it: the PQ curve, BT.2020 primaries, at
  least 10 bits per channel, and static metadata sent with the picture (`HDR_OUTPUT_METADATA`).
- **Headless mode**: Yuna running without a physical screen, on virtual outputs.
- **Image description**: in `color-management-v1`, what a surface's pixel values mean or what an
  output expects: a transfer function, primaries, luminances and a target colour volume. Each
  has an **identity**, a number no other ever has, so that a client tells a copy from a new one
  ([ADR-0047](adr/0047-color-management-v1-kwins-list.md)).
- **Hidden window**: a window on a workspace not shown, or in a column scrolled out of view; it gets
  at most one frame callback a second ([ADR-0023](adr/0023-frames-presentation-and-the-clock.md)).
- **Home output**: the output a workspace goes back to when that output returns after being
  unplugged ([ADR-0018](adr/0018-workspaces.md)).
- **lavapipe**: Mesa's Vulkan driver that draws on the CPU; the applications' tests give it to
  every Vulkan client.
- **Layer**: the part of a workspace a window is placed in: the strip of scrolling columns, the
  tiles of dwindle, the floating windows, or the pinned ones above them
  ([ADR-0052](adr/0052-motion-in-a-pure-crate.md)).
- **llvmpipe**: Mesa's OpenGL driver that draws on the CPU; the headless mode's default
  ([ADR-0022](adr/0022-the-headless-backend-in-yuna.md)).
- **Logical coordinates**: the layout's units, shared by all outputs: an output of 3840 × 2160
  pixels at scale 1.5 measures 2560 × 1440 logical units. Rectangles are snapped to the
  **physical grid**, the output's own pixels.
- **Look** (MCP): the MCP server's comparison of what the compositor describes of windows and
  outputs with what it described last, from which it tells their events; run once a round and
  at each `events` call, only when the compositor's description generation has moved.
- **Managed window**: an X11 window its manager places, as Yuna's layout does an `xdg_toplevel`:
  one that asks to be mapped, neither override-redirect nor a dock or a desktop
  ([ADR-0066](adr/0066-yunas-own-x11-window-manager-on-x11rb.md)).
- **Manager's slice**: the part of a round of Yuna's loop in which it handles its X11 managers'
  events: until shortly before the next deadline, 8 ms at most; the rest wait for the next
  rounds, which come at once.
- **Manual clock**: the compositor's clock in tests, and in `yuna mcp --manual-clock`: it starts at
  the real monotonic time and moves only when a test, or an agent through the `clock` tool,
  advances it, running each deadline it passes, in order
  ([ADR-0023](adr/0023-frames-presentation-and-the-clock.md)). An agent never moves it past real
  time.
- **MCP**: the Model Context Protocol, through which an agent's client calls a server's tools;
  `yuna mcp` is one, on its standard input and output ([ADR-0033](adr/0033-the-mcp-server.md)).
- **Menu** (X11): an override-redirect window Yuna shows, a menu, a tooltip or a list: drawn with
  the window the rules give it, just above it and sliding with it as a popup does, or, with none,
  **alone**, at its own X11 position over every window
  ([ADR-0066](adr/0066-yunas-own-x11-window-manager-on-x11rb.md)).
- **Motion**: a value moving to a target on a spring or a curve, a function of time alone, held by
  `yuna-motion`; it ends when it can no longer move a pixel
  ([ADR-0053](adr/0053-springs-in-closed-form-keeping-their-velocity.md)).
- **Newest frame**: what a capture shows: the frame an output's armed refresh draws, else the
  one its buffer holds.
- **Offset**: a window's drawn position minus its place, after a layout operation moved it; it
  decays to 0 on a spring ([ADR-0052](adr/0052-motion-in-a-pure-crate.md)).
- **Override-redirect**: said of an X11 window its client places itself, past its manager:
  menus, tooltips, a drag's icon.
- **Parent** (bench): a compositor that another runs inside, in the same sandbox, counted in the
  stack, not the core: Hyprland's headless dwl, which lends it a render node.
- **Pace** (bench): how a backend times the frames of a compositor whose client draws as fast as
  it can, as each recipe records it: wlroots' headless timer, a frame 4 ms after each commit, or
  the mode's rate. In `game` and `pacing`, costs a second rank only within one pace.
- **Pace** (MCP): how much of the time a program's process group may run: free, frozen until
  thawed, or slowed to a share of each 100 ms (`spawn freeze`, `thaw`, `slow`).
- **Pairing**: matching an X11 window with the Wayland surface Xwayland draws it into, through
  the serial `xwayland-shell-v1` gives both, in either order.
- **Peers** (bench): the rivals with Yuna's features, niri, Hyprland, Umbriel and KWin: Yuna's
  budgets of a frame's cost and of memory are their best figure at equal work, measured the same
  day ([ADR-0072](adr/0072-budgets-set-against-the-peers-the-same-day.md)).
- **Pinned window**: a floating window shown over whichever workspace its output shows.
- **Popup grab**: a popup's request for its seat's keyboard and pointer until it is dismissed, as
  an open menu holds them; granted only with a serial its client's own seat sent to its window,
  and only to a popup not dismissed. While it holds, the keyboard stays within its window, and a
  click outside the client's surfaces dismisses it.
- **Portal**: a D-Bus service an application calls to ask the desktop for something, such as
  sharing the screen; those that need the compositor are served by `yuna-portal`, a program of
  its own beside Yuna ([ADR-0057](adr/0057-what-yuna-does-and-what-the-shell-does.md)).
- **Lead**: how long before a refresh boundary an output starts drawing its frame: its longest
  drawing of the last half second, at least 2 ms, plus 1 ms, at most one period
  ([ADR-0031](adr/0031-frames-drawn-ahead-of-their-boundary.md)). An output is **idle** when the
  boundary after its last refresh has passed; it then draws a commit at once.
- **PQ**: SMPTE ST 2084, the perceptual quantizer: the curve that turns an absolute luminance, up
  to 10,000 cd/m², into an HDR signal. A **PQ code** is that signal as a 10-bit integer, 0 to
  1,023.
- **Preferred description**: the image description Yuna tells a surface to draw in, its
  window's workspace's output's ([ADR-0048](adr/0048-hdr-virtual-outputs-pq-announced-drawn-at-stage-5.md)).
- **Primary selection**: the text last selected, pasted with the middle button; a second selection
  beside the clipboard, from X11.
- **Program** (MCP): a process an agent started through `spawn`, with its log; what it starts in
  turn stays under Yuna, its **subreaper**, and ends with it.
- **Recipe**: how the rivals' bench runs one compositor, as data in `crates/yuna-bench`: its
  program, environment and configuration, how it is ready, stopped and driven, and how its frames
  are counted.
- **Refresh boundary**: the moment an output could start showing a new frame; a virtual output's
  come every `1 / rate` seconds of Yuna's clock ([ADR-0023](adr/0023-frames-presentation-and-the-clock.md)).
  A VRR output has none: its panel starts a refresh when a frame is ready, within its range.
- **Refused** (bench): said of a case whose sandbox broke its rules, or whose scene was not the
  scenario's (mpv not fullscreen): the bench stops it and measures nothing. A refusal is not a
  failure.
- **Retarget**: giving a running motion a new target; a spring goes on from its value and its
  velocity ([ADR-0053](adr/0053-springs-in-closed-form-keeping-their-velocity.md)).
- **Run file**: the versioned JSON file a run of the rivals' bench writes, every figure with its
  unit; `yuna-bench report` summarises run files.
- **Sandbox** (bench): what each compositor and client of the rivals' bench runs in: a systemd
  scope of its own, an environment built from nothing, a runtime folder and session bus of its
  own. It detects a process that reaches the owner's display, devices, runtime folder or bus, and
  refuses its case; it prevents nothing ([ARCHITECTURE](ARCHITECTURE.md#the-rivals-bench)).
- **Seat**: a set of input devices, a pointer and a keyboard, with its own focus. Each agent has
  one, apart from the owner's, with its own outputs and Wayland socket; a client belongs to the
  seat whose socket it came through ([ADR-0035](adr/0035-agents-seats.md)). Its **handle** is
  the number `seat create` gives, which every tool but `clock` takes.
- **Selection**: what a client offers to paste, as a list of MIME types: the clipboard or the
  primary selection. Each seat has its own.
- **Sentinel**: in the compositor's tests of misbehaving clients and random requests, a
  well-behaved client that joined first; its frame callbacks, at the exact deadline of its frame,
  show that the others did not stop its frames.
- **Share picker**: the selection Yuna draws when an application asks to share the screen, the same
  as for captures; choosing is the consent
  ([ADR-0057](adr/0057-what-yuna-does-and-what-the-shell-does.md)).
- **Special workspace**: a hidden workspace shown over the current one by a key.
- **State thread**: the thread that runs Yuna's event loop and owns its state; a panic there is
  fatal, unlike one on the X11 manager's or an encoder's thread.
- **Strip**: in scrolling columns, the row of columns of a workspace; the **view** is the part of
  it the output shows, and scrolls to keep the focused column whole.
- **Suspended**: an `xdg_toplevel` state telling a client its window cannot be seen; Yuna sends it
  to hidden windows, to clients whose `xdg_wm_base` version has it (6 and later).
- **Sync request**: `_NET_WM_SYNC_REQUEST`, a value an X11 window is sent before a configure that
  resizes it, which it sets in its sync counter once it drew the new size; meanwhile Xwayland
  holds its commits, for 100 ms at most, so that its frame shows whole.
- **Theme file**: a colours-only file a shell or theme tool writes and Yuna's configuration
  includes, reloaded live; its format is a public interface
  ([ADR-0057](adr/0057-what-yuna-does-and-what-the-shell-does.md)).
- **Tour**: the owner's run of the rivals on a real display, by hand, from a spare virtual terminal,
  one after another ([ADR-0075](adr/0075-the-rivals-measured-headless-now-on-a-real-display-at-stage-2s-exit.md)).
  A rival that cannot run headless, niri or cosmic-comp, is measured there alone: **tour only**.
- **Unresponsive**: said of an X11 window asked to close that left its ping (`_NET_WM_PING`)
  unanswered 5 s; it stays until it answers one, never ended on a timer.
- **Urgent**: said of a window that asked for the focus and did not get it, because its request
  came without a valid activation token or a fullscreen window held the screen; it stays urgent
  until it gets the focus or closes. Nothing shows it yet.
- **Variant**: a named set of settings in the configuration, applied over it and switched on
  command, such as the shell's compact mode
  ([ADR-0057](adr/0057-what-yuna-does-and-what-the-shell-does.md)).
- **Virtual keyboard** (bench): the bench's client that types key chords through
  `zwp_virtual_keyboard_v1` into one compositor's sandboxed instance, for one driven by its keys
  alone, dwl; its keys reach no other compositor.
- **Virtual output**: an output no monitor shows, drawn offscreen at its own size, rate and scale
  ([ADR-0022](adr/0022-the-headless-backend-in-yuna.md)).
- **VRR**: variable refresh rate; the monitor refreshes when a frame is ready rather than at a fixed
  rate.
- **Wake-up**: a round of the compositor's event loop in which something had work for it: a
  client, a new connection or the clock's timer. At rest there are none.
  In the rivals' bench, a voluntary context switch of a thread, which the kernel counts for any
  process: each time the thread slept, and was woken later.
- **Working area**: the part of an output tiled windows go in; the whole output until panels
  reserve zones of it.
- **XWayland's scale**: S, the largest scale among a seat's outputs when its Xwayland started,
  fixed for that Xwayland's life: how many X11 pixels a logical unit is
  ([ADR-0068](adr/0068-x11-windows-drawn-by-yuna-through-an-area-filter.md)).
