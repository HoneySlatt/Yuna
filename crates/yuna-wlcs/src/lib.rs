//! Yuna's integration module for WLCS, the Wayland conformance suite: a shared library that
//! WLCS 1.8.1 loads, so that its tests run against Yuna headless (ADR-0149).
//!
//! **How WLCS loads it.** `wlcs libyuna_wlcs.so [gtest options]` opens the library into its own
//! process (`dlopen`) and reads the one symbol it exports, `wlcs_server_integration`
//! (`hooks.rs`). For each test it makes a server, starts it, connects its clients through the
//! descriptors the server gives, places windows and moves a pointer through the server's hooks,
//! then stops and frees it. The library's C side is declared in [`abi`], from WLCS's ABI, with no
//! text of its headers (GPL-3.0-only); `tests/abi.rs` checks it against them. Two calls of
//! libwayland-client's, `wl_display_get_fd` and `wl_proxy_get_id`, are bound at load time to the
//! copy WLCS links, which its clients use.
//!
//! **Threads.** Each server runs one thread, `yuna-wlcs-server`, which owns a Yuna
//! [`Compositor`](yuna::Compositor) and sleeps in its loop (`server.rs`). Hooks run on WLCS's
//! threads: each sends its request through a channel of one, wakes the server's thread
//! ([`Waker`](yuna::Waker)) and waits for the reply. The thread lets Yuna settle, applies the
//! request, lets Yuna settle again, and replies: settled is no deadline due within two refreshes,
//! and each of the two settles waits 0.5 s at most ([`compositor::WlcsCompositor::settle`]). A
//! reply not given in 10 s, and Yuna not started in as long, abort the process, naming what went
//! unanswered; `stop` returns once the thread has ended, in 5 s, else the process is aborted.
//!
//! **What it runs.** Yuna on the CPU, on real time, with no socket, no Xwayland and no
//! animation; one agent's seat, whose new windows open floating so that WLCS may place them, and
//! one output, 1920 × 1080 at 60 Hz and scale 1, at the logical origin ([`compositor`]). The
//! seat's globals and their versions are what [`descriptor`] tells WLCS, which skips the tests
//! of the others.
//!
//! **Its limits.** Yuna has no touch: a touch device does nothing but print the marker
//! `yuna-wlcs: unsupported: touch` on standard error, once a server, and a request Yuna refuses
//! by design prints its own, `yuna-wlcs: unsupported: <what>`; the runner files a test that
//! printed one as unsupported, never as passing. When the shim itself goes wrong, it prints
//! `yuna-wlcs: error: <what>`, and the runner fails the test whatever it expects. Yuna runs on
//! real time: WLCS waits in real time, which Yuna's manual clock cannot follow, the one task of
//! Yuna's tests not run on it (ADR-0082, item 6). One pointer: every pointer device WLCS makes
//! moves the seat's one. No keyboard: WLCS 1.8.1 has no hook for one.
//!
//! **What it never does.** It never reaches a display, a GPU, the network or the owner's seat,
//! and takes no option from WLCS's command line. It catches no panic: a panic, in Yuna or in a
//! hook, prints its message and aborts the process, as a panic of Yuna's state is fatal
//! (ADR-0080). It names no type of Smithay's (`scripts/check.sh`): it drives Yuna through its
//! interface and the test seams of ADR-0149.

pub mod abi;
pub mod compositor;
pub mod descriptor;
mod hooks;
mod server;
