//! What Yuna owes a manager: the commands its loop sent that the manager's thread has not taken
//! yet, in the order sent, shared between the two under a lock (`mod.rs`'s `Commands` on Yuna's
//! side, `commands.rs`'s `take_commands` on the thread's).
//!
//! A manager that stops taking them, its Xwayland stopped or hung, must not make Yuna's memory
//! grow with each round. So the commands of a kind whose latest says all it sets, the focus, the
//! stacking and each selection, replace the one owed ([`Latest`]): a Wayland client that changes
//! its clipboard at each round is owed one. And what is owed is bounded, about, in bytes
//! ([`MAX_OWED_BYTES`]): past it, Yuna takes the manager as stalled and ends its Xwayland. Before
//! that, a watchdog on Yuna's side ends it once it has taken no command for
//! [`MANAGER_STALL`](crate::xwayland::MANAGER_STALL) ([`Owed::watch`]). While that watchdog
//! waits, the manager tells Yuna when it takes what it owes ([`Taken::watched`]), so that the
//! watchdog is disarmed at once and nothing is left armed at rest.

use std::mem::size_of;
use std::time::Instant;

use x11rb::protocol::xproto::Window;

use super::messages::Command;
use crate::xwayland::bridge::Selection;

/// The most bytes, about, the commands Yuna owes one manager may hold: what a stalled manager can
/// cost Yuna, beside what it held when it stalled. One round owes a manager at most about three
/// commands for each X11 window, mapped, placed and told its states: 30,000 for the 10,000
/// windows Yuna holds at most (`MAX_WINDOWS`), 1.2 MB in commands of 40 bytes on x86-64. 8 MiB
/// holds more than five such rounds that a manager's thread did not reach, which a loaded machine
/// never makes it wait.
pub(crate) const MAX_OWED_BYTES: usize = 8 * 1024 * 1024;

/// The variable that, in a debug build, lowers the bound on what Yuna owes each X11 manager to
/// the bytes it gives, for the test of what happens at that bound, which a test could not
/// otherwise reach before the manager's watchdog
/// ([ADR-0080](../../../../../docs/adr/0080-what-survives-a-failure-and-yunas-own-panics.md)).
/// It is read once, at the first manager of the process, and holds for every manager of the
/// process; a value that is not a number, or not below the bound, changes nothing. A release
/// build has neither the variable nor the lower bound.
#[cfg(debug_assertions)]
pub const X11_OWED_BOUND: &str = "YUNA_X11_OWED_BOUND";

/// The bound on what a manager is owed: [`MAX_OWED_BYTES`], or in a debug build the lower one
/// [`X11_OWED_BOUND`] gives.
pub(super) fn owed_bound() -> usize {
    #[cfg(debug_assertions)]
    {
        static BOUND: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
        *BOUND.get_or_init(|| {
            std::env::var(X11_OWED_BOUND)
                .ok()
                .and_then(|value| value.parse::<usize>().ok())
                .map_or(MAX_OWED_BYTES, |bound| bound.min(MAX_OWED_BYTES))
        })
    }
    #[cfg(not(debug_assertions))]
    MAX_OWED_BYTES
}

/// What one command owed takes in the list, beside what it points to.
const SLOT: usize = size_of::<Option<Command>>();

/// The kinds of command whose latest says all it sets: the X input focus, the stacking of every
/// window, and each selection's Wayland source. The latest replaces the one owed, and goes in
/// its own place in the order: what was sent between is carried out before it, as it was sent.
#[derive(Debug, Clone, Copy)]
enum Latest {
    Focus,
    Restack,
    Clipboard,
    Primary,
}

impl Latest {
    const COUNT: usize = 4;

    fn of(command: &Command) -> Option<Latest> {
        match command {
            Command::Focus(_) => Some(Latest::Focus),
            Command::Restack(_) => Some(Latest::Restack),
            Command::OwnSelection { selection, .. } => Some(match selection {
                Selection::Clipboard => Latest::Clipboard,
                Selection::Primary => Latest::Primary,
            }),
            _ => None,
        }
    }
}

/// A command sent past the bound on what a manager is owed, its figure in bytes: the manager is
/// taken as stalled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct OverBound {
    pub(crate) bound: usize,
}

/// What the manager took of what it was owed ([`Owed::take`]).
#[derive(Debug, Default)]
pub(super) struct Taken {
    /// Every command it was owed, in the order sent.
    pub(super) commands: Vec<Command>,
    /// Whether Yuna's watchdog waited on them ([`Owed::watch`]): the manager then tells Yuna it
    /// took them.
    pub(super) watched: bool,
}

/// What Yuna owes a manager.
#[derive(Debug)]
pub(super) struct Owed {
    /// The commands in the order sent; a place emptied held one a later one of its kind
    /// replaced, and stays counted until the manager takes the list.
    commands: Vec<Option<Command>>,
    /// The place in `commands` of the one command owed of each [`Latest`] kind.
    latest: [Option<usize>; Latest::COUNT],
    /// What `commands` holds, about: each place, and the text and lists each command points to.
    bytes: usize,
    /// The most `bytes` may reach: [`MAX_OWED_BYTES`], lower in a test ([`X11_OWED_BOUND`]).
    bound: usize,
    /// When the oldest command the manager has not taken was sent, replaced or not.
    untaken_since: Option<Instant>,
    /// Whether Yuna's watchdog waits on the commands owed now: set by [`Owed::watch`], and
    /// cleared when the manager takes them, which it then tells Yuna.
    watched: bool,
    /// Whether Yuna let the manager go: its thread ends.
    released: bool,
}

impl Owed {
    /// Nothing owed, bounded at `bound` bytes.
    pub(super) fn bounded_at(bound: usize) -> Owed {
        Owed {
            commands: Vec::new(),
            latest: [None; Latest::COUNT],
            bytes: 0,
            bound,
            untaken_since: None,
            watched: false,
            released: false,
        }
    }

    /// Adds `command`, sent at `now`; a command of a [`Latest`] kind replaces the one owed.
    /// Refused, and nothing changed, when what is owed would pass its bound.
    pub(super) fn push(&mut self, command: Command, now: Instant) -> Result<(), OverBound> {
        let latest = Latest::of(&command);
        let replaced = latest.and_then(|kind| self.latest[kind as usize]);
        let freed = replaced
            .and_then(|at| self.commands[at].as_ref())
            .map_or(0, heap_bytes);
        let bytes = self.bytes - freed + SLOT + heap_bytes(&command);
        if bytes > self.bound {
            return Err(OverBound { bound: self.bound });
        }
        if let Some(at) = replaced {
            self.commands[at] = None;
        }
        if let Some(kind) = latest {
            self.latest[kind as usize] = Some(self.commands.len());
        }
        self.commands.push(Some(command));
        self.bytes = bytes;
        self.untaken_since.get_or_insert(now);
        Ok(())
    }

    /// Every command owed, in the order sent, the manager owing none from then on; and whether
    /// Yuna's watchdog waited on them, which it waits on no longer.
    pub(super) fn take(&mut self) -> Taken {
        if self.commands.is_empty() {
            return Taken::default();
        }
        self.latest = [None; Latest::COUNT];
        self.bytes = 0;
        self.untaken_since = None;
        let commands = std::mem::take(&mut self.commands)
            .into_iter()
            .flatten()
            .collect();
        Taken {
            commands,
            watched: std::mem::take(&mut self.watched),
        }
    }

    /// When the oldest command the manager has not taken was sent; none when it owes none.
    /// While it owes some, Yuna's watchdog waits on them: the manager tells Yuna when it takes
    /// them ([`Taken::watched`]).
    pub(super) fn watch(&mut self) -> Option<Instant> {
        self.watched = self.untaken_since.is_some();
        self.untaken_since
    }

    /// Lets the manager go, and frees what it was owed.
    pub(super) fn release(&mut self) {
        self.released = true;
        self.take();
    }

    /// Whether Yuna let the manager go.
    pub(super) fn released(&self) -> bool {
        self.released
    }
}

/// What `command` points to beyond its own size, about: text and lists.
fn heap_bytes(command: &Command) -> usize {
    let strings = |strings: &[String]| -> usize {
        strings
            .iter()
            .map(|text| size_of::<String>() + text.len())
            .sum()
    };
    match command {
        Command::Restack(windows) => windows.len() * size_of::<Window>(),
        Command::OwnSelection { mime_types, .. } => mime_types.as_deref().map_or(0, strings),
        Command::StartWaylandDrag { mime_types, .. } => strings(mime_types),
        Command::SendToWayland { mime_type, .. } | Command::SendX11Drag { mime_type, .. } => {
            mime_type.len()
        }
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::xwayland::manager::NetState;

    fn own(serial: u64, types: usize) -> Command {
        Command::OwnSelection {
            selection: Selection::Clipboard,
            serial,
            mime_types: Some(vec!["x".repeat(255); types]),
        }
    }

    /// A short name for what is taken, in order.
    fn named(commands: &[Command]) -> Vec<String> {
        commands
            .iter()
            .map(|command| match command {
                Command::Map(window) => format!("map {window}"),
                Command::Focus(window) => format!("focus {window:?}"),
                Command::Restack(order) => format!("restack {order:?}"),
                Command::OwnSelection { serial, .. } => format!("own {serial}"),
                other => format!("{other:?}"),
            })
            .collect()
    }

    #[test]
    fn the_latest_of_a_kind_replaces_the_one_owed_and_takes_its_own_place() {
        let mut owed = Owed::bounded_at(MAX_OWED_BYTES);
        let now = Instant::now();
        let later = now + Duration::from_secs(1);
        owed.push(Command::Focus(Some(1)), now).expect("owed");
        owed.push(Command::Map(2), now).expect("owed");
        owed.push(Command::Restack(vec![1]), now).expect("owed");
        owed.push(own(1, 64), now).expect("owed");
        owed.push(Command::Focus(Some(2)), later).expect("owed");
        owed.push(Command::Restack(vec![1, 2]), later)
            .expect("owed");
        owed.push(own(2, 64), later).expect("owed");
        // The primary selection is a kind of its own.
        owed.push(
            Command::OwnSelection {
                selection: Selection::Primary,
                serial: 3,
                mime_types: None,
            },
            later,
        )
        .expect("owed");
        assert_eq!(owed.untaken_since, Some(now), "since the first, replaced");
        let taken = owed.take().commands;
        assert_eq!(
            named(&taken)[..4],
            ["map 2", "focus Some(2)", "restack [1, 2]", "own 2"]
        );
        assert_eq!(taken.len(), 5);
        assert_eq!(owed.untaken_since, None);
        assert!(owed.take().commands.is_empty());
    }

    #[test]
    fn the_manager_tells_yuna_it_took_only_what_a_watchdog_waited_on() {
        let mut owed = Owed::bounded_at(MAX_OWED_BYTES);
        let now = Instant::now();
        // Nothing owed: no watchdog waits.
        assert_eq!(owed.watch(), None);
        owed.push(Command::Map(1), now).expect("owed");
        assert!(!owed.take().watched, "taken before a watchdog waited");
        owed.push(Command::Map(2), now).expect("owed");
        assert_eq!(owed.watch(), Some(now));
        let taken = owed.take();
        assert_eq!(named(&taken.commands), ["map 2"]);
        assert!(taken.watched, "the watchdog that waited is told");
        // Told once: what is owed next has no watchdog until one is armed for it.
        owed.push(Command::Map(3), now).expect("owed");
        assert!(!owed.take().watched);
        // A manager let go tells nothing.
        owed.push(Command::Map(4), now).expect("owed");
        owed.watch();
        owed.release();
        assert_eq!(owed.watch(), None);
        assert!(!owed.take().watched);
    }

    #[test]
    fn a_clipboard_changed_again_and_again_stays_one_command() {
        let mut owed = Owed::bounded_at(MAX_OWED_BYTES);
        let now = Instant::now();
        for serial in 0..10_000 {
            owed.push(own(serial, 64), now).expect("owed");
        }
        // One list of 64 types, and an emptied place for each change.
        assert!(
            owed.bytes < 10_000 * SLOT + 64 * 300,
            "{} bytes",
            owed.bytes
        );
        assert_eq!(named(&owed.take().commands), ["own 9999"]);
    }

    #[test]
    fn what_is_owed_stops_at_its_bound_and_refuses_the_rest_whole() {
        let mut owed = Owed::bounded_at(MAX_OWED_BYTES);
        let now = Instant::now();
        let mut sent = 0;
        while owed.push(Command::Map(sent), now).is_ok() {
            sent += 1;
        }
        assert!(owed.bytes <= MAX_OWED_BYTES);
        assert!(owed.bytes + SLOT > MAX_OWED_BYTES);
        assert_eq!(sent as usize, MAX_OWED_BYTES / SLOT);
        // Refused, a replacement changes nothing either.
        let over = Err(OverBound {
            bound: MAX_OWED_BYTES,
        });
        assert_eq!(owed.push(Command::Focus(None), now), over);
        assert_eq!(owed.take().commands.len(), sent as usize);
        // Taken, the manager is owed anew.
        assert_eq!(owed.push(Command::Map(0), now), Ok(()));
    }

    #[test]
    fn a_lower_bound_refuses_sooner() {
        let mut owed = Owed::bounded_at(4 * SLOT);
        let now = Instant::now();
        for window in 0..4 {
            owed.push(Command::Map(window), now).expect("owed");
        }
        assert_eq!(
            owed.push(Command::Map(4), now),
            Err(OverBound { bound: 4 * SLOT })
        );
    }

    #[test]
    fn the_bound_holds_five_rounds_telling_every_window_yuna_may_hold() {
        let round = 3 * crate::MAX_WINDOWS;
        assert!(
            5 * round * SLOT <= MAX_OWED_BYTES,
            "a command takes {SLOT} bytes"
        );
        let state = Command::State {
            window: 1,
            state: NetState::default(),
        };
        assert_eq!(heap_bytes(&state), 0);
    }

    #[test]
    fn a_manager_let_go_is_owed_nothing() {
        let mut owed = Owed::bounded_at(MAX_OWED_BYTES);
        owed.push(own(1, 64), Instant::now()).expect("owed");
        owed.release();
        assert!(owed.released());
        assert_eq!(owed.untaken_since, None);
        assert!(owed.take().commands.is_empty());
    }
}
