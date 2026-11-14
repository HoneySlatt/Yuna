# 0067. One XWayland per seat, on demand, behind a cookie

- Status: Accepted
- Date: 2026-11-14

## Context

X11 has no isolation between its clients: any one of them can read another's pixels
(`GetImage`), send it input (`SendEvent`, XTEST) and read its selections. Rootless Xwayland
admits any process of the same user when it is started without `-auth` (`os/access.c`).
[ADR-0015](0015-agents-workspaces.md) says an agent's seat never reaches the owner's. In Yuna, a
client's seat is the socket it came through ([ADR-0035](0035-agents-seats.md)).

KWin and Mutter start Xwayland on the first X11 connection and pass `-auth`; sway and wlroots
start it lazily; Hyprland at once. Xwayland exits a delay after its last client with
`-terminate <delay>`, but only when every client left, its window manager included, has asked
XFixes to be disconnected (`dix/dispatch.c`, KWin and Mutter do it). With `-wm`, Xwayland
listens on its sockets only once a client owns `WM_S0` (`hw/xwayland/xwayland.c`). libxcb tries
the abstract socket `@/tmp/.X11-unix/X<n>` first.

## Decision

The owner chose, on 2026-11-14: **one Xwayland per seat, started on the first X11 connection,
stopped when idle, each behind a cookie of its own.**

- Each seat that may run programs gets a display number when it is created: the first free one,
  reserved by its abstract socket, which the kernel frees when Yuna dies, then its file socket in
  `/tmp/.X11-unix` and its lock file. Its programs get `DISPLAY` and `XAUTHORITY`.
- Each has an MIT-MAGIC-COOKIE-1 of 16 random bytes, in an Xauthority file of the seat's own,
  passed to Xwayland with `-auth`.
- Yuna watches the sockets; the first connection spawns Xwayland with them (`-listenfd`),
  rootless, with `-terminate 10`, on a Wayland connection Yuna records as a client of that seat.
  The waiting client is served once Xwayland is ready.
- The start's order: Yuna waits for `-displayfd`, connects its manager, sets XFixes'
  disconnection mode, `RESOURCE_MANAGER` and XSETTINGS, and takes `WM_S0` last, so that no client
  reads a setting before it exists. Nothing in it blocks Yuna's loop.
- Xwayland exits, crashes, or closes `-displayfd` without a number: its windows close, Yuna logs
  it and watches the sockets again; a crash loop backs off. Xwayland missing: logged once, no
  `DISPLAY`.
- Xwayland is one Wayland client for all of a seat's X11 programs: Yuna counts windows per X11
  client ([ADR-0028](0028-bounds-on-what-one-client-may-ask.md)), and never posts Xwayland a
  protocol error for what one X11 client did.

## Alternatives considered

- **One Xwayland for the whole session**: one server, less memory when several seats use X11 at
  once. But an agent's X11 program could read and drive the owner's, which ADR-0015 forbids, and
  Yuna would have to guess the seat of each X11 window and choose one scale for seats whose
  outputs differ. The owner first chose it, then came back to one per seat.
- **One per seat, never stopped**: no start time after a pause, but a server runs for nothing.
  Mutter keeps its Xwayland by default, because a restart loses the user's `xrdb` and `xset`
  state: whether the owner's seat stops when idle is decided with the desktop, at Stage 2.

## Consequences

- At rest, a seat without X11 programs has no Xwayland; nothing polls.
- A program of the same user that reads another seat's cookie file can still connect to it, as it
  can open another seat's Wayland socket: the cookie stops mistakes, not a hostile process of the
  same user.
- `yuna --headless` prints `DISPLAY=:<n>`; MCP's programs get their seat's.
- The greeter mode has no XWayland ([ADR-0065](0065-what-hollow-asks-of-yuna.md)).
