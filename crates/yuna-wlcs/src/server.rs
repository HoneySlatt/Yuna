//! One WLCS server: the thread that owns its compositor, and the link every hook reaches it
//! through.
//!
//! **The thread**, `yuna-wlcs-server`, is made by `start` and owns the [`WlcsCompositor`],
//! which never leaves it (Yuna's compositor is not `Send`). It sleeps in Yuna's loop until a
//! client, a deadline or a hook wakes it: nothing polls.
//!
//! **The link**: a channel that holds one request at most, and the compositor's
//! [`Waker`]. A hook sends its request, wakes the thread, and waits for the reply,
//! which the thread sends once Yuna has settled from what came before, the request is applied,
//! and Yuna has settled from it ([`WlcsCompositor::settle`]). WLCS calls one hook at a time, so
//! the channel is empty when a
//! hook sends; were two hooks to call at once, the second would wait for the first's turn.
//!
//! **Stopping**: `stop` asks the thread to end, and waits [`STOP_LIMIT`] at most for it to
//! drop the compositor; past it, the shim aborts the process, as WLCS requires a server torn
//! down before the next test begins. A device kept past its server's stop gets no reply, and
//! says so.
//!
//! **Unsupported requests** print WLCS's marker, `yuna-wlcs: unsupported: <what>`, on standard
//! error, once a server for each kind ([`Markers`]): the test runner files a test that printed
//! it as unsupported, never as passing.

use std::collections::BTreeSet;
use std::io::{self, Write};
use std::os::fd::RawFd;
use std::os::unix::net::UnixStream;
use std::sync::mpsc::{Receiver, RecvTimeoutError, SyncSender, sync_channel};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::JoinHandle;
use std::time::Duration;

use yuna::Waker;

use crate::compositor::{Unsupported, WlcsCompositor};

/// How long `stop` waits for the server's thread to end before it aborts the process.
pub const STOP_LIMIT: Duration = Duration::from_secs(5);

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
    Unsupported(Unsupported),
    /// Yuna could not do it: a connection refused, or its loop failed, which ends the server.
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
                Some(why) => report(&format!("yuna-wlcs: unsupported: {what}: {why}")),
                None => report(&format!("yuna-wlcs: unsupported: {what}")),
            }
        }
    }
}

/// Writes `line` to standard error, as WLCS reads the module's: if that fails, nothing is left
/// to tell.
pub fn report(line: &str) {
    let _ = writeln!(io::stderr().lock(), "{line}");
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
    /// Sends `request`, wakes the thread and waits for its reply; none once the thread ended.
    fn ask(&self, request: Request) -> Option<Reply> {
        let (reply, answer) = sync_channel(1);
        self.requests.send(Message::Ask { request, reply }).ok()?;
        self.waker.wake();
        answer.recv().ok()
    }

    /// Asks `request`, whose reply carries nothing, and tells what went wrong.
    fn act(&self, request: Request) {
        match self.ask(request) {
            Some(Reply::Done) => {}
            Some(Reply::Unsupported(unsupported)) => {
                self.markers
                    .unsupported(unsupported.what, Some(&unsupported.why));
            }
            Some(Reply::Failed(error)) => report(&format!("yuna-wlcs: {error}")),
            Some(Reply::Client(_)) => report("yuna-wlcs: a client was made unasked, and dropped"),
            None => report("yuna-wlcs: the server has stopped: the request is dropped"),
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

    /// The markers of the server it links to.
    pub fn markers(&self) -> &Arc<Markers> {
        &self.markers
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

    /// Starts the server's thread, and returns once its compositor runs. Refused when it runs
    /// already, and when Yuna does not start: the server then stays stopped, and each hook
    /// reports that it is.
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
        let waker = match started.recv() {
            Ok(Ok(waker)) => waker,
            Ok(Err(error)) => {
                let _ = thread.join();
                return Err(error);
            }
            Err(_) => {
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
        match running.ended.recv_timeout(STOP_LIMIT) {
            Ok(()) | Err(RecvTimeoutError::Disconnected) => {
                // The thread has dropped what it held: it returns at once.
                let _ = running.thread.join();
            }
            Err(RecvTimeoutError::Timeout) => {
                report(&format!(
                    "yuna-wlcs: the server did not stop in {STOP_LIMIT:?}: WLCS cannot go on"
                ));
                std::process::abort();
            }
        }
    }

    /// A new client's end of a connection, for WLCS to take.
    pub fn connect_client(&self) -> Result<UnixStream, String> {
        let link = self.link().ok_or("the server is not running")?;
        match link.ask(Request::ConnectClient) {
            Some(Reply::Client(stream)) => Ok(stream),
            Some(Reply::Failed(error)) => Err(error),
            Some(reply) => Err(format!("the server answered {reply:?} to a connection")),
            None => Err("the server has stopped".to_owned()),
        }
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
            None => report("yuna-wlcs: the server is not running: no window is placed"),
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
            report(&format!(
                "yuna-wlcs: Yuna's loop failed: {error}; the server stops"
            ));
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
/// gives the reply; an error is a failure of Yuna's loop, the server's end. Settling first draws
/// what clients committed since the last frame, which a hook's pointer is then tested against,
/// as a person's would be once the frame showed: WLCS moves the pointer right after a client's
/// commit and one roundtrip.
fn answer(wlcs: &mut WlcsCompositor, request: Request) -> Result<Reply, yuna::Error> {
    wlcs.settle()?;
    let applied = match request {
        Request::ConnectClient => match wlcs.connect_client() {
            Ok(stream) => Ok(Some(stream)),
            Err(error) => return Ok(Reply::Failed(format!("no client connects: {error}"))),
        },
        Request::PlaceWindow {
            client,
            surface,
            x,
            y,
        } => wlcs.place_window(client, surface, x, y).map(|()| None),
        Request::MovePointerTo { x, y } => wlcs.move_pointer_to(x, y).map(|()| None),
        Request::MovePointerBy { dx, dy } => wlcs.move_pointer_by(dx, dy).map(|()| None),
        Request::PressButton { button, pressed } => {
            wlcs.press_button(button, pressed).map(|()| None)
        }
    };
    wlcs.settle()?;
    Ok(match applied {
        Ok(Some(stream)) => Reply::Client(stream),
        Ok(None) => Reply::Done,
        Err(unsupported) => Reply::Unsupported(unsupported),
    })
}
