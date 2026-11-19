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
//! [`MANAGER_STALL`](crate::xwayland::MANAGER_STALL) ([`Owed::untaken_since`]).

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

/// The commands sent past [`MAX_OWED_BYTES`]: the manager is taken as stalled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct OverBound;

/// What Yuna owes a manager.
#[derive(Debug, Default)]
pub(super) struct Owed {
    /// The commands in the order sent; a place emptied held one a later one of its kind
    /// replaced, and stays counted until the manager takes the list.
    commands: Vec<Option<Command>>,
    /// The place in `commands` of the one command owed of each [`Latest`] kind.
    latest: [Option<usize>; Latest::COUNT],
    /// What `commands` holds, about: each place, and the text and lists each command points to.
    bytes: usize,
    /// When the oldest command the manager has not taken was sent, replaced or not.
    untaken_since: Option<Instant>,
    /// Whether Yuna let the manager go: its thread ends.
    released: bool,
}

impl Owed {
    /// Adds `command`, sent at `now`; a command of a [`Latest`] kind replaces the one owed.
    /// Refused, and nothing changed, when what is owed would pass [`MAX_OWED_BYTES`].
    pub(super) fn push(&mut self, command: Command, now: Instant) -> Result<(), OverBound> {
        let latest = Latest::of(&command);
        let replaced = latest.and_then(|kind| self.latest[kind as usize]);
        let freed = replaced
            .and_then(|at| self.commands[at].as_ref())
            .map_or(0, heap_bytes);
        let bytes = self.bytes - freed + SLOT + heap_bytes(&command);
        if bytes > MAX_OWED_BYTES {
            return Err(OverBound);
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

    /// Every command owed, in the order sent, the manager owing none from then on.
    pub(super) fn take(&mut self) -> Vec<Command> {
        if self.commands.is_empty() {
            return Vec::new();
        }
        self.latest = [None; Latest::COUNT];
        self.bytes = 0;
        self.untaken_since = None;
        std::mem::take(&mut self.commands)
            .into_iter()
            .flatten()
            .collect()
    }

    /// When the oldest command the manager has not taken was sent; none when it owes none.
    pub(super) fn untaken_since(&self) -> Option<Instant> {
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
        let mut owed = Owed::default();
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
        assert_eq!(owed.untaken_since(), Some(now), "since the first, replaced");
        let taken = owed.take();
        assert_eq!(
            named(&taken)[..4],
            ["map 2", "focus Some(2)", "restack [1, 2]", "own 2"]
        );
        assert_eq!(taken.len(), 5);
        assert_eq!(owed.untaken_since(), None);
        assert!(owed.take().is_empty());
    }

    #[test]
    fn a_clipboard_changed_again_and_again_stays_one_command() {
        let mut owed = Owed::default();
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
        assert_eq!(named(&owed.take()), ["own 9999"]);
    }

    #[test]
    fn what_is_owed_stops_at_its_bound_and_refuses_the_rest_whole() {
        let mut owed = Owed::default();
        let now = Instant::now();
        let mut sent = 0;
        while owed.push(Command::Map(sent), now).is_ok() {
            sent += 1;
        }
        assert!(owed.bytes <= MAX_OWED_BYTES);
        assert!(owed.bytes + SLOT > MAX_OWED_BYTES);
        assert_eq!(sent as usize, MAX_OWED_BYTES / SLOT);
        // Refused, a replacement changes nothing either.
        assert_eq!(owed.push(Command::Focus(None), now), Err(OverBound));
        assert_eq!(owed.take().len(), sent as usize);
        // Taken, the manager is owed anew.
        assert_eq!(owed.push(Command::Map(0), now), Ok(()));
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
        let mut owed = Owed::default();
        owed.push(own(1, 64), Instant::now()).expect("owed");
        owed.release();
        assert!(owed.released());
        assert_eq!(owed.untaken_since(), None);
        assert!(owed.take().is_empty());
    }
}
