//! One WLCS server: the thread that owns its compositor, and the link every hook reaches it
//! through.
//!
//! **The thread**, `yuna-wlcs-server`, is made by `start` and owns the [`WlcsCompositor`],
//! which never leaves it (Yuna's compositor is not `Send`). It sleeps in Yuna's loop until a
//! client, a deadline or a hook wakes it: nothing polls.
//!
//! **The link**: a channel that holds one request at most, and the compositor's [`Waker`]. A
//! hook sends its request, wakes the thread, and waits for the reply, which the thread sends once
//! Yuna has settled from what came before, the request is applied, and Yuna has settled from it
//! ([`WlcsCompositor::settle`]). WLCS calls one hook at a time, so the channel is empty when a
//! hook sends; were two hooks to call at once, the second would wait for the first's turn. A
//! hook waits [`REPLY_LIMIT`] for its reply, and `start` as long for Yuna to start: past it, the
//! thread is stuck, and WLCS with the hook; the shim names what went unanswered and aborts the
//! process.
//!
//! **Stopping**: `stop` asks the thread to end, and waits [`STOP_LIMIT`] at most for it to
//! drop the compositor; past it, the shim aborts the process, as WLCS requires a server torn
//! down before the next test begins. A device kept past its server's stop gets no reply, and
//! says so.
//!
//! **Markers**, each one line on standard error, written in one write so that no other output
//! cuts it, which the test runner reads between a test's start and its end:
//!
//! - `yuna-wlcs: unsupported: <what>`: a request Yuna refuses by design, once a server for each
//!   kind ([`Markers`]). The runner files the test unsupported, never passing.
//! - `yuna-wlcs: error: <what>` ([`report_error`]): the shim itself went wrong, or WLCS asked
//!   what it never should, each time. The test's outcome cannot be trusted: the runner fails it,
//!   whatever it is expected to give.

use std::collections::BTreeSet;
use std::io::{self, Write};
use std::os::fd::RawFd;
use std::os::unix::net::UnixStream;
use std::sync::mpsc::{Receiver, RecvTimeoutError, SyncSender, sync_channel};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::JoinHandle;
use std::time::Duration;

use yuna::Waker;

use crate::compositor::{NotDone, Unsupported, WlcsCompositor};

/// How long `stop` waits for the server's thread to end before it aborts the process.
pub const STOP_LIMIT: Duration = Duration::from_secs(5);

/// How long a hook waits for the server's reply, and `start` for Yuna to start, before the shim
/// aborts the process: WLCS's own waits give up after as long. A request takes two settles of
/// half a second at most, and its own rounds ([`crate::compositor::SETTLE_LIMIT`]).
pub const REPLY_LIMIT: Duration = Duration::from_secs(10);

/// The start of the marker of a request Yuna refuses by design.
const UNSUPPORTED_MARKER: &str = "yuna-wlcs: unsupported: ";

/// The start of the marker of the shim's own error.
const ERROR_MARKER: &str = "yuna-wlcs: error: ";

/// What a hook asks the server's thread.
#[derive(Debug)]
enum Request {
    ConnectClient,
    PlaceWindow {
        client: RawFd,
        surface: u32,
        x: i32,
        y: i32,
    },
    MovePointerTo {
        x: f64,
        y: f64,
    },
    MovePointerBy {
        dx: f64,
        dy: f64,
    },
    PressButton {
        button: i32,
        pressed: bool,
    },
}

/// What the server's thread answers.
#[derive(Debug)]
enum Reply {
    /// A client's end of its new connection.
    Client(UnixStream),
    Done,
    /// Yuna refuses it by design.
    Unsupported(Unsupported),
    /// The shim went wrong, or Yuna's loop failed, which ends the server: in words.
    Failed(String),
}

/// What the server's thread is sent.
enum Message {
    /// A request, with where its reply goes: a channel of its own, for one reply.
    Ask {
        request: Request,
        reply: SyncSender<Reply>,
    },
    /// The end of the thread.
    Stop,
}

/// The markers a server printed, so that each prints once.
#[derive(Debug, Default)]
pub struct Markers {
    printed: Mutex<BTreeSet<&'static str>>,
}

impl Markers {
    /// Prints `yuna-wlcs: unsupported: <what>` on standard error, with `why` after it when there
    /// is one, unless this server printed it already.
    pub fn unsupported(&self, what: &'static str, why: Option<&str>) {
        let first = self
            .printed
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(what);
        if first {
            match why {
                Some(why) => write_line(&format!("{UNSUPPORTED_MARKER}{what}: {why}\n")),
                None => write_line(&format!("{UNSUPPORTED_MARKER}{what}\n")),
            }
        }
    }
}

/// Prints `yuna-wlcs: error: <what>` on standard error: the shim went wrong, or was asked what
/// it never should be, so that the test's outcome cannot be trusted.
pub fn report_error(what: &str) {
    write_line(&format!("{ERROR_MARKER}{what}\n"));
}

/// Writes `line`, its newline included, to standard error in one write, as WLCS reads the
/// module's: standard error is not buffered, and a line written in parts could be cut by another
/// thread's output, which would hide a marker from the runner. A pipe never splits a write of
/// 4,096 bytes or fewer. If the write fails, nothing is left to tell.
fn write_line(line: &str) {
    let _ = io::stderr().lock().write_all(line.as_bytes());
}

/// Waits `limit` at most for what `answer` receives about `asked`, the request or step named in
/// words; none once its sender is gone. Past the limit, the thread that should answer is stuck,
/// and WLCS with the hook that waits on it: the shim says what went unanswered and aborts the
/// process.
fn await_reply<T>(answer: &Receiver<T>, limit: Duration, asked: &str) -> Option<T> {
    match answer.recv_timeout(limit) {
        Ok(reply) => Some(reply),
        Err(RecvTimeoutError::Disconnected) => None,
        Err(RecvTimeoutError::Timeout) => {
            report_error(&format!(
                "the server's thread did not answer {asked} in {limit:?}: WLCS cannot go on"
            ));
            std::process::abort();
        }
    }
}

/// How a hook reaches a running server's thread. Devices keep one, which outlives the server
/// harmlessly: once its thread has ended, nothing answers.
#[derive(Clone)]
pub struct Link {
    requests: SyncSender<Message>,
    waker: Waker,
    markers: Arc<Markers>,
}

impl Link {
    /// Sends `request`, wakes the thread and waits [`REPLY_LIMIT`] for its reply, past which the
    /// process is aborted; none once the thread ended.
    fn ask(&self, request: Request) -> Option<Reply> {
        let asked = format!("{request:?}");
        let (reply, answer) = sync_channel(1);
        self.requests.send(Message::Ask { request, reply }).ok()?;
        self.waker.wake();
        await_reply(&answer, REPLY_LIMIT, &asked)
    }

    /// Asks `request`, whose reply carries nothing, and tells what went wrong.
    fn act(&self, request: Request) {
        match self.ask(request) {
            Some(Reply::Done) => {}
            Some(Reply::Unsupported(unsupported)) => {
                self.markers
                    .unsupported(unsupported.what, Some(&unsupported.why));
            }
            Some(Reply::Failed(error)) => report_error(&error),
            Some(Reply::Client(_)) => report_error("a client was made unasked, and dropped"),
            None => report_error("the server has stopped: the request is dropped"),
        }
    }

    /// Moves the seat's pointer to (`x`, `y`).
    pub fn move_pointer_to(&self, x: f64, y: f64) {
        self.act(Request::MovePointerTo { x, y });
    }

    /// Moves the seat's pointer by (`dx`, `dy`).
    pub fn move_pointer_by(&self, dx: f64, dy: f64) {
        self.act(Request::MovePointerBy { dx, dy });
    }

    /// Presses or releases the seat's pointer `button`.
    pub fn press_button(&self, button: i32, pressed: bool) {
        self.act(Request::PressButton { button, pressed });
    }
}

/// A server's thread, while it runs.
struct Running {
    link: Link,
    thread: JoinHandle<()>,
    /// Disconnected once the thread has ended, its compositor dropped: it is never sent to.
    ended: Receiver<()>,
}

/// One WLCS server: made by `create_server`, started and stopped once by WLCS.
#[derive(Default)]
pub struct Server {
    markers: Arc<Markers>,
    running: Mutex<Option<Running>>,
}

impl Server {
    /// A server, not yet started.
    pub fn new() -> Server {
        Server::default()
    }

    /// The markers this server prints.
    pub fn markers(&self) -> &Arc<Markers> {
        &self.markers
    }

    /// Starts the server's thread, and returns once its compositor runs, which [`REPLY_LIMIT`]
    /// bounds: past it, the process is aborted. Refused when it runs already, and when Yuna does
    /// not start: the server then stays stopped, and each hook reports that it is.
    pub fn start(&self) -> Result<(), String> {
        let mut running = self.running.lock().unwrap_or_else(PoisonError::into_inner);
        if running.is_some() {
            return Err("the server runs already".to_owned());
        }
        let (requests, received) = sync_channel(1);
        let (ready, started) = sync_channel(1);
        let (ended_sender, ended) = sync_channel(0);
        let thread = std::thread::Builder::new()
            .name("yuna-wlcs-server".to_owned())
            .spawn(move || serve(&received, &ready, ended_sender))
            .map_err(|error| format!("the server's thread did not start: {error}"))?;
        let waker = match await_reply(&started, REPLY_LIMIT, "Yuna's start") {
            Some(Ok(waker)) => waker,
            Some(Err(error)) => {
                let _ = thread.join();
                return Err(error);
            }
            None => {
                let _ = thread.join();
                return Err("the server's thread ended before it started".to_owned());
            }
        };
        *running = Some(Running {
            link: Link {
                requests,
                waker,
                markers: self.markers.clone(),
            },
            thread,
            ended,
        });
        Ok(())
    }

    /// The link to the server's thread while it runs.
    pub fn link(&self) -> Option<Link> {
        self.running
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .map(|running| running.link.clone())
    }

    /// Ends the server's thread and returns once it has ended, its compositor dropped, which
    /// [`STOP_LIMIT`] bounds: past it, the process is aborted. Nothing happens to a server
    /// stopped.
    pub fn stop(&self) {
        let running = self
            .running
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        let Some(running) = running else {
            return;
        };
        // A thread already ended has dropped the channel's end: there is nothing to stop.
        if running.link.requests.send(Message::Stop).is_ok() {
            running.link.waker.wake();
        }
        // Past the limit, the process is aborted; else the thread has dropped what it held, and
        // returns at once.
        await_reply(&running.ended, STOP_LIMIT, "the stop");
        let _ = running.thread.join();
    }

    /// A new client's end of a connection, for WLCS to take; none when no client connects,
    /// which its marker says: unsupported when Yuna refuses the client, an error otherwise.
    pub fn connect_client(&self) -> Option<UnixStream> {
        let Some(link) = self.link() else {
            report_error("no client socket: the server is not running");
            return None;
        };
        match link.ask(Request::ConnectClient) {
            Some(Reply::Client(stream)) => return Some(stream),
            Some(Reply::Unsupported(unsupported)) => {
                self.markers
                    .unsupported(unsupported.what, Some(&unsupported.why));
            }
            Some(Reply::Failed(error)) => report_error(&format!("no client socket: {error}")),
            Some(Reply::Done) => report_error("no client socket: the server answered none"),
            None => report_error("no client socket: the server has stopped"),
        }
        None
    }

    /// Places the window of `client`'s `surface` at (`x`, `y`).
    pub fn place_window(&self, client: RawFd, surface: u32, x: i32, y: i32) {
        match self.link() {
            Some(link) => link.act(Request::PlaceWindow {
                client,
                surface,
                x,
                y,
            }),
            None => report_error("the server is not running: no window is placed"),
        }
    }
}

impl Drop for Server {
    /// A server freed while it runs is stopped first, so that no thread outlives it.
    fn drop(&mut self) {
        self.stop();
    }
}

/// The server's thread: starts the compositor, gives `ready` its waker, or why it did not
/// start, then runs its loop and answers each request, until a stop or a failure of the loop.
/// `ended` drops last, after the compositor: its drop tells `stop` the thread is done. A
/// connection refused is told to its hook, and the server goes on.
fn serve(
    requests: &Receiver<Message>,
    ready: &SyncSender<Result<Waker, String>>,
    ended: SyncSender<()>,
) {
    let _ended = ended;
    let mut wlcs = match WlcsCompositor::start() {
        Ok(wlcs) => wlcs,
        Err(error) => {
            let _ = ready.send(Err(error));
            return;
        }
    };
    match wlcs.waker() {
        Ok(waker) => {
            let _ = ready.send(Ok(waker));
        }
        Err(error) => {
            let _ = ready.send(Err(format!("no waker: {error}")));
            return;
        }
    }
    loop {
        if let Err(error) = wlcs.dispatch() {
            report_error(&format!("Yuna's loop failed: {error}; the server stops"));
            return;
        }
        while let Ok(message) = requests.try_recv() {
            let Message::Ask { request, reply } = message else {
                return;
            };
            // A hook gone, which cannot happen while it waits, takes no reply.
            match answer(&mut wlcs, request) {
                Ok(answered) => {
                    let _ = reply.send(answered);
                }
                Err(error) => {
                    let error = format!("Yuna's loop failed: {error}; the server stops");
                    let _ = reply.send(Reply::Failed(error));
                    return;
                }
            }
        }
    }
}

/// Lets Yuna settle from what came before, applies `request`, lets Yuna settle from it, and
/// gives the reply; each settle waits [`SETTLE_LIMIT`](crate::compositor::SETTLE_LIMIT) at most.
/// An error is a failure of Yuna's loop, the server's end. Settling first draws what clients
/// committed since the last frame, which a hook's pointer is then tested against, as a person's
/// would be once the frame showed: WLCS moves the pointer right after a client's commit and one
/// roundtrip.
fn answer(wlcs: &mut WlcsCompositor, request: Request) -> Result<Reply, yuna::Error> {
    wlcs.settle()?;
    let applied = match request {
        Request::ConnectClient => wlcs.connect_client().map(Some),
        Request::PlaceWindow {
            client,
            surface,
            x,
            y,
        } => wlcs.place_window(client, surface, x, y).map(|()| None),
        Request::MovePointerTo { x, y } => wlcs
            .move_pointer_to(x, y)
            .map(|()| None)
            .map_err(NotDone::Unsupported),
        Request::MovePointerBy { dx, dy } => wlcs
            .move_pointer_by(dx, dy)
            .map(|()| None)
            .map_err(NotDone::Unsupported),
        Request::PressButton { button, pressed } => wlcs
            .press_button(button, pressed)
            .map(|()| None)
            .map_err(NotDone::Unsupported),
    };
    wlcs.settle()?;
    Ok(match applied {
        Ok(Some(stream)) => Reply::Client(stream),
        Ok(None) => Reply::Done,
        Err(NotDone::Unsupported(unsupported)) => Reply::Unsupported(unsupported),
        Err(NotDone::ShimError(error)) => Reply::Failed(error),
    })
}

#[cfg(test)]
mod tests {
    use std::os::unix::process::ExitStatusExt;
    use std::process::Command;

    use super::*;

    /// Marks the process the abort runs in: this test binary, run again, filtered to the test.
    const ABORTING: &str = "YUNA_WLCS_TEST_ABORTING";

    /// SIGABRT, which `abort` raises.
    const SIGABRT: i32 = 6;

    #[test]
    fn a_reply_not_given_in_time_names_the_request_and_aborts_the_process() {
        if std::env::var_os(ABORTING).is_some() {
            // The sender lives on and never sends: the thread that should answer is stuck.
            let (_stuck, answer) = sync_channel::<Reply>(1);
            let asked = format!("{:?}", Request::MovePointerTo { x: 1.5, y: 2.0 });
            await_reply(&answer, Duration::from_millis(100), &asked);
            panic!("await_reply returned past its limit");
        }
        let test =
            "server::tests::a_reply_not_given_in_time_names_the_request_and_aborts_the_process";
        let program = std::env::current_exe().expect("the test binary");
        // Through a shell that turns core files off: the abort is expected.
        let aborted = Command::new("sh")
            .args(["-c", "ulimit -c 0 && exec \"$0\" \"$@\""])
            .arg(program)
            .args([test, "--exact", "--nocapture", "--test-threads=1"])
            .env(ABORTING, "1")
            .output()
            .expect("the test runs in a process of its own");
        let stderr = String::from_utf8_lossy(&aborted.stderr);
        assert_eq!(aborted.status.signal(), Some(SIGABRT), "{stderr}");
        assert!(
            stderr.contains(
                "yuna-wlcs: error: the server's thread did not answer MovePointerTo { x: 1.5, y: \
                 2.0 } in 100ms: WLCS cannot go on\n"
            ),
            "{stderr}"
        );
    }

    #[test]
    fn a_reply_given_or_its_sender_gone_returns() {
        let (reply, answer) = sync_channel(1);
        reply.send(Reply::Done).expect("the reply is sent");
        assert!(matches!(
            await_reply(&answer, REPLY_LIMIT, "a test"),
            Some(Reply::Done)
        ));
        drop(reply);
        assert!(await_reply(&answer, REPLY_LIMIT, "a test").is_none());
    }
}
