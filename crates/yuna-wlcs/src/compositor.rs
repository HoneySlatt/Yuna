//! The compositor a WLCS server runs, on the server's thread alone: Yuna headless, one agent's
//! seat whose windows open floating, and one output at the origin of the logical space, so that
//! WLCS's coordinates are the output's. Yuna is let settle before each request of a hook is
//! applied, and again before the hook answers ([`WlcsCompositor::settle`]).

use std::collections::HashMap;
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::net::UnixStream;
use std::time::{Duration, Instant};

use yuna::{ClientKey, ClockMode, Compositor, Config, Device, Error, OutputMode, Waker};
use yuna_types::{OutputId, SeatId};

/// The output's width, in physical pixels and logical units alike, at scale 1.
pub const OUTPUT_WIDTH: u32 = 1920;
/// The output's height.
pub const OUTPUT_HEIGHT: u32 = 1080;
/// The output's rate, in refreshes a second.
pub const OUTPUT_RATE: f64 = 60.0;

/// The longest a hook's request waits for Yuna to settle: past it, the hook answers anyway.
pub const SETTLE_LIMIT: Duration = Duration::from_millis(500);

/// A request a hook could not have honoured, and why: what the shim prints as WLCS's marker
/// of an unsupported test (`server.rs`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unsupported {
    /// What was asked, the same words for every request of its kind.
    pub what: &'static str,
    /// Why Yuna refused it, in Yuna's words.
    pub why: String,
}

impl Unsupported {
    fn new(what: &'static str, why: impl ToString) -> Unsupported {
        Unsupported {
            what,
            why: why.to_string(),
        }
    }
}

/// Yuna's compositor as one WLCS server runs it, with the seat and output it made, and the
/// clients it connected, by the descriptor of their end of the connection.
pub struct WlcsCompositor {
    compositor: Compositor,
    seat: SeatId,
    output: OutputId,
    /// One refresh of the output: what [`WlcsCompositor::settle`] waits past.
    refresh: Duration,
    /// Each client connected, by the descriptor WLCS was given for it, which its `wl_display`
    /// reads back. WLCS closes a descriptor when its client goes, and the number may come back
    /// for the next: the newest client of a number replaces the one before. At most the clients
    /// a test connects, each seat's bound with them (`MAX_CLIENTS_PER_SEAT`).
    clients: HashMap<RawFd, ClientKey>,
}

impl WlcsCompositor {
    /// Starts Yuna as a WLCS server runs it: on the CPU (llvmpipe), on real time, with no socket,
    /// no Xwayland and no animation; one agent's seat, whose new windows open floating, and its
    /// output, 1920 × 1080 at 60 Hz and scale 1, which must be at the logical origin. Returns
    /// once its globals are announced and its first frame drawn.
    pub fn start() -> Result<WlcsCompositor, String> {
        let mut compositor = Compositor::new(Config {
            outputs: Vec::new(),
            device: Device::Software,
            clock: ClockMode::Real,
            listen: false,
            // An agent seat's clients are shown no other seat's `wl_seat`, whatever this says;
            // `yuna mcp` announces none either.
            announce_owner_seat: false,
            xwayland: None,
            ..Config::default()
        })
        .map_err(|error| format!("Yuna did not start: {error}"))?;
        let seat = compositor
            .add_seat()
            .map_err(|error| format!("no agent seat: {error}"))?;
        let mode = OutputMode::new(OUTPUT_WIDTH, OUTPUT_HEIGHT, OUTPUT_RATE, 1.0)
            .map_err(|error| format!("no output mode: {error}"))?;
        let refresh = mode.period();
        let output = compositor
            .add_output(seat, mode)
            .map_err(|error| format!("no output: {error}"))?;
        compositor
            .set_seat_opens_floating(seat, true)
            .map_err(|refusal| format!("the seat's windows cannot float: {refusal}"))?;
        let mut wlcs = WlcsCompositor {
            compositor,
            seat,
            output,
            refresh,
            clients: HashMap::new(),
        };
        wlcs.settle()
            .map_err(|error| format!("Yuna's loop failed: {error}"))?;
        wlcs.check_output_at_origin()?;
        Ok(wlcs)
    }

    /// WLCS places windows and the pointer in one logical space, which the shim takes as the
    /// output's own: it holds only while the output's corner is at the origin.
    fn check_output_at_origin(&self) -> Result<(), String> {
        let rect = self
            .compositor
            .output_descriptions()
            .into_iter()
            .find(|output| output.id == self.output)
            .map(|output| output.rect)
            .ok_or("the output is gone")?;
        if (rect.x, rect.y) != (0.0, 0.0) {
            return Err(format!(
                "the output is at ({}, {}), not at the origin",
                rect.x, rect.y
            ));
        }
        Ok(())
    }

    /// The compositor, for the tests that read what the shim's offers.
    pub fn compositor(&mut self) -> &mut Compositor {
        &mut self.compositor
    }

    /// The agent's seat every client of the server belongs to.
    pub fn seat(&self) -> SeatId {
        self.seat
    }

    /// A handle that wakes the server's thread from a hook's.
    pub fn waker(&mut self) -> Result<Waker, Error> {
        self.compositor.waker()
    }

    /// Runs one round of Yuna's loop, waiting for what comes, with no limit: the server's loop.
    pub fn dispatch(&mut self) -> Result<(), Error> {
        self.compositor.dispatch(None)
    }

    /// Lets Yuna settle, before and after a request: one round at once, which sends what Yuna
    /// wrote, then rounds until no deadline is due within one refresh, and at most
    /// [`SETTLE_LIMIT`]. Yuna draws what clients committed, and a window where it was placed, at
    /// the next refresh, and hit tests read the last frame drawn; it gives the keyboard's focus
    /// and a window's activation at the refresh after a click. WLCS checks right after a hook and
    /// one roundtrip, so a hook's request waits for the frame before it, and its answer for the
    /// frame after it. A deadline beyond one refresh, a hidden window's frame callbacks for
    /// instance, is not waited for.
    pub fn settle(&mut self) -> Result<(), Error> {
        let started = Instant::now();
        self.compositor.dispatch(Some(Duration::ZERO))?;
        while let Some(due) = self.compositor.next_deadline() {
            let now = self.compositor.now();
            if due > now + self.refresh {
                break;
            }
            let Some(left) = SETTLE_LIMIT.checked_sub(started.elapsed()) else {
                tracing::debug!("Yuna did not settle in {SETTLE_LIMIT:?}; the hook answers");
                break;
            };
            self.compositor
                .dispatch(Some(due.saturating_sub(now).min(left)))?;
        }
        Ok(())
    }

    /// Connects a client of the seat: its end of the connection, which WLCS takes.
    pub fn connect_client(&mut self) -> Result<UnixStream, Error> {
        let (key, stream) = self.compositor.connect_client_keyed(self.seat)?;
        self.clients.insert(stream.as_raw_fd(), key);
        Ok(stream)
    }

    /// Places the window of the `wl_surface` of protocol id `surface` of the client connected
    /// through `client`, its top-left corner at (`x`, `y`) on the output. Unsupported for a
    /// client the shim did not connect, a surface that is no mapped window, and a window Yuna
    /// does not place: tiled, or not shown.
    pub fn place_window(
        &mut self,
        client: RawFd,
        surface: u32,
        x: i32,
        y: i32,
    ) -> Result<(), Unsupported> {
        let key = self.clients.get(&client).ok_or_else(|| {
            Unsupported::new(
                "placing a window of a client the shim did not connect",
                format!("descriptor {client}"),
            )
        })?;
        let window = self
            .compositor
            .window_of_surface(key, surface)
            .ok_or_else(|| {
                Unsupported::new(
                    "placing a surface that is no mapped window",
                    format!("wl_surface@{surface}"),
                )
            })?;
        self.compositor
            .place_window(window, f64::from(x), f64::from(y))
            .map_err(|refusal| Unsupported::new("placing a window Yuna refuses to place", refusal))
    }

    /// Moves the seat's pointer to (`x`, `y`) on the output. Unsupported off the output, which
    /// Yuna refuses.
    pub fn move_pointer_to(&mut self, x: f64, y: f64) -> Result<(), Unsupported> {
        self.compositor
            .pointer_move(self.seat, Some(self.output), x, y)
            .map(drop)
            .map_err(|error| Unsupported::new("a pointer move Yuna refuses", error))
    }

    /// Moves the seat's pointer by (`dx`, `dy`) from where it is, from the output's corner
    /// before it first moved.
    pub fn move_pointer_by(&mut self, dx: f64, dy: f64) -> Result<(), Unsupported> {
        let pointer = self.compositor.pointer(self.seat);
        self.move_pointer_to(pointer.x + dx, pointer.y + dy)
    }

    /// Presses or releases the seat's pointer `button`, a Linux input code. Unsupported for a
    /// code below 0, and for a press of a button held or a release of one not held, which Yuna
    /// refuses.
    pub fn press_button(&mut self, button: i32, pressed: bool) -> Result<(), Unsupported> {
        let code = u32::try_from(button)
            .map_err(|_| Unsupported::new("a button Yuna refuses", format!("code {button}")))?;
        self.compositor
            .pointer_button(self.seat, code, pressed)
            .map(drop)
            .map_err(|error| Unsupported::new("a button Yuna refuses", error))
    }
}
