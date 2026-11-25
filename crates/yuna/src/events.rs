//! The compositor's events
//! ([ADR-0153](../../../docs/adr/0153-one-event-stream-marks-at-the-change-mcps-log-per-seat-the-bound.md)):
//! marks where what an event tells changes, and the build that turns them into
//! `yuna_ipc::Event`s, the one place that makes them.
//!
//! Nothing is marked, kept or built until someone watches (`Compositor::watch_events`): the MCP
//! server watches from its start. Unwatched, each change point costs one test of an `Option`; a
//! command reads nothing for the events ([`EventMarks::watching`]).
//!
//! **Marks.** Each change point calls in here with what changed, never a description:
//!
//! - a window opened (`shell.rs`, `State::map`) or closed (`State::close`); one that closes
//!   before a build told it opened is read at its close, so that the build tells it opened then
//!   closed;
//! - its title or application id (`shell.rs`'s `title_changed` and `app_id_changed`, and
//!   `xwayland/windows.rs`'s `x11_properties_changed`), and whether it answers pings
//!   (`xwayland/answers.rs`);
//! - where it rests: the arrangement (`frames.rs`, `State::arrange`) marks each window whose
//!   [`Placed`] rectangle or workspace changed, or that came into or left an output's
//!   arrangement; a window shown on another output, or on none (`State::move_to_output`,
//!   `State::drop_output`); the window a command moves to a workspace or along its strip, and
//!   the windows of a workspace that loses its name ([`moved_by`], read before `command.rs` runs
//!   the command);
//! - an output added, configured, moved or removed (`output.rs`);
//! - the focus, at each change of the layout (`State::mark_layout_changed`);
//! - a client that left (`Compositor::free_departed_clients`) or connections refused
//!   (`seats.rs`), each kept as its event at once, in the order they came.
//!
//! A mark repeats cheaply: a map by window, emptied by each build with its capacity kept, so that
//! a round at rest, or one in which clients only draw, marks and allocates nothing.
//!
//! **The build** ([`take`]) turns the marks into events, comparing each fact marked with what it
//! last told, kept for each window, output and seat, the texts cut to [`MAX_EVENT_TEXT`]: a fact
//! changed and changed back tells nothing, one changed a thousand times one event. It reads the
//! windows marked alone, counted in [`EventMarks::windows_described`]. A window's place is built
//! once no arrangement is pending; until then its mark waits. A placed window's workspace is
//! named from its [`Placed::workspace`] (`Layout::workspace_name`, whose cost follows the
//! workspaces); [`Layout::window`](yuna_layout::Layout::window), whose cost grows with the
//! windows, is asked only for a window marked that no output arranges. A build that places
//! windows reads each output's arrangement once, a lookup per window it holds. Its order is
//! `yuna_ipc::Event`'s; [`watch`] marks every window and output there is then, so that the
//! first build tells them.
//!
//! **Bounds.** What was told: one entry per open window, at most
//! [`MAX_WINDOWS`](crate::MAX_WINDOWS), with three texts of [`MAX_EVENT_TEXT`] at most; one per
//! output, at most [`MAX_OUTPUTS`](crate::MAX_OUTPUTS); one per seat that focuses a window. The
//! marks: the windows marked since the last build, the open ones and those opened and closed
//! meanwhile, each of the latter with two texts of [`MAX_EVENT_TEXT`]; the clients that left and
//! the refusals since. The MCP server builds at the end of each round, so that these hold what one
//! round brought; a caller that watches and never builds keeps them growing.
//!
//! **Limits.** A window's process id, shell and texts are read when a build first tells it, or at
//! its close: a window whose client left before is told opened without its process id, and an X11
//! window destroyed before it was unmapped, or whose Xwayland ended, without its texts, Yuna's
//! record of it gone. A change of a window's process id after it opened is not told.

use std::collections::hash_map::Entry;
use std::collections::{HashMap, HashSet};

use yuna_ipc::{Command, Event, MAX_EVENT_TEXT, ProtocolError};
use yuna_layout::Placed;
use yuna_types::{OutputId, Rect, SeatId, WindowId, WorkspaceId};

use crate::MAX_CLIENTS_PER_SEAT;
use crate::compositor::OutputDescription;
use crate::shell::{Toplevel, WindowFacts};
use crate::state::State;

/// The marks since the last build and what was told, once someone watches; none before.
#[derive(Debug, Default)]
pub(crate) struct EventMarks(Option<Marks>);

#[derive(Debug, Default)]
struct Marks {
    /// The windows marked since the last build, by id.
    windows: HashMap<WindowId, WindowMark>,
    /// The outputs marked since the last build: added, changed or removed, as the build finds.
    outputs: HashSet<OutputId>,
    /// Whether a seat's focus may have moved since the last build.
    focus: bool,
    /// The clients that left and the refusals since the last build, in the order they came.
    clients: Vec<Event>,
    told: Told,
    /// How many windows the builds read, since watching began.
    described: u64,
    /// The marked windows' ids, by id, for a build: its capacity kept.
    order: Vec<WindowId>,
    /// Windows' places in their outputs' arrangements, for the arrangement's comparison and a
    /// build: its capacity kept.
    places: HashMap<WindowId, Arranged>,
}

/// What may have changed of a window since the last build.
#[derive(Debug, Default)]
struct WindowMark {
    /// It opened, on this seat.
    opened: Option<SeatId>,
    /// Its title or application id.
    texts: bool,
    /// Where it rests: kept while an arrangement is pending.
    place: bool,
    /// Whether it answers pings.
    answers: bool,
    /// It closed.
    closed: bool,
    /// What it was, read at its close when no build had told it opened, its texts cut.
    facts: Option<WindowFacts>,
}

/// What the builds told, the texts cut as they were told.
#[derive(Debug, Default)]
struct Told {
    windows: HashMap<WindowId, ToldWindow>,
    outputs: HashMap<OutputId, OutputDescription>,
    /// The window each seat focuses, for the seats that focus one.
    focus: HashMap<SeatId, WindowId>,
}

/// What the builds told of an open window.
#[derive(Debug)]
struct ToldWindow {
    seat: SeatId,
    title: Option<String>,
    app_id: Option<String>,
    unresponsive: bool,
    /// Where it rests: nowhere until its first `WindowPlaced`.
    placement: Placement,
}

/// Where a window rests, as `WindowPlaced` tells it.
#[derive(Debug, Default, PartialEq)]
struct Placement {
    output: Option<OutputId>,
    workspace: Option<String>,
    place: Option<Rect>,
}

/// A window's place in an output's arrangement.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Arranged {
    output: OutputId,
    rect: Rect,
    workspace: WorkspaceId,
}

impl Arranged {
    fn of(output: OutputId, placed: &Placed) -> Arranged {
        Arranged {
            output,
            rect: placed.rect,
            workspace: placed.workspace,
        }
    }
}

/// `text`'s first [`MAX_EVENT_TEXT`] bytes at most, cut at a character's boundary.
fn cut(text: &str) -> &str {
    let mut end = text.len().min(MAX_EVENT_TEXT);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

/// `text` as an event gives it, [`cut`].
fn bounded(text: Option<&str>) -> Option<String> {
    text.map(|text| cut(text).to_owned())
}

impl EventMarks {
    /// Whether someone watches: what a change point whose mark costs more than a test asks
    /// first.
    pub(crate) fn watching(&self) -> bool {
        self.0.is_some()
    }

    /// The mark of `window`, made empty if it has none; none while nobody watches.
    fn window(&mut self, window: WindowId) -> Option<&mut WindowMark> {
        self.0
            .as_mut()
            .map(|marks| marks.windows.entry(window).or_default())
    }

    /// `window` opened on `seat`.
    pub(crate) fn window_opened(&mut self, window: WindowId, seat: SeatId) {
        if let Some(mark) = self.window(window) {
            mark.opened = Some(seat);
        }
    }

    /// `window`'s title or application id may have changed.
    pub(crate) fn window_texts_changed(&mut self, window: WindowId) {
        if let Some(mark) = self.window(window) {
            mark.texts = true;
        }
    }

    /// Where `window` rests may have changed: its output, its workspace or its place.
    pub(crate) fn window_moved(&mut self, window: WindowId) {
        if let Some(mark) = self.window(window) {
            mark.place = true;
        }
    }

    /// Whether `window` answers pings may have changed.
    pub(crate) fn window_answers_changed(&mut self, window: WindowId) {
        if let Some(mark) = self.window(window) {
            mark.answers = true;
        }
    }

    /// Whether `window` opened since the last build, which has not told it: what its close
    /// reads, so that the build tells it opened then closed.
    pub(crate) fn awaits_opening(&self, window: WindowId) -> bool {
        self.0.as_ref().is_some_and(|marks| {
            marks
                .windows
                .get(&window)
                .is_some_and(|mark| mark.opened.is_some())
        })
    }

    /// `window` closed; `facts`, what it was, when no build told it opened
    /// ([`EventMarks::awaits_opening`]).
    pub(crate) fn window_closed(&mut self, window: WindowId, facts: Option<WindowFacts>) {
        if let Some(mark) = self.window(window) {
            mark.closed = true;
            mark.facts = facts.map(|facts| WindowFacts {
                app_id: bounded(facts.app_id.as_deref()),
                title: bounded(facts.title.as_deref()),
                ..facts
            });
        }
    }

    /// `output` was added, configured, moved or removed.
    pub(crate) fn output_changed(&mut self, output: OutputId) {
        if let Some(marks) = &mut self.0 {
            marks.outputs.insert(output);
        }
    }

    /// A seat's focus may have moved: the layout changed.
    pub(crate) fn focus_may_have_moved(&mut self) {
        if let Some(marks) = &mut self.0 {
            marks.focus = true;
        }
    }

    /// `output` was arranged again, from `before` to `after`: marks each window whose rectangle
    /// or workspace changed, or that came into or left its arrangement. In the same order, as an
    /// arrangement that moved nothing gives, it compares pairwise; else through a map by window,
    /// its capacity kept.
    pub(crate) fn arranged(&mut self, output: OutputId, before: &[Placed], after: &[Placed]) {
        let Some(marks) = &mut self.0 else {
            return;
        };
        let same_windows = before.len() == after.len()
            && before
                .iter()
                .zip(after)
                .all(|(old, new)| old.window == new.window);
        if same_windows {
            for (old, new) in before.iter().zip(after) {
                if Arranged::of(output, old) != Arranged::of(output, new) {
                    marks.windows.entry(new.window).or_default().place = true;
                }
            }
            return;
        }
        marks.places.clear();
        marks.places.extend(
            before
                .iter()
                .map(|placed| (placed.window, Arranged::of(output, placed))),
        );
        for placed in after {
            if marks.places.remove(&placed.window) != Some(Arranged::of(output, placed)) {
                marks.windows.entry(placed.window).or_default().place = true;
            }
        }
        for &left in marks.places.keys() {
            marks.windows.entry(left).or_default().place = true;
        }
        marks.places.clear();
    }

    /// A client of `seat`, its number `client`, process `pid`, left, ended for `error` or not.
    pub(crate) fn client_left(
        &mut self,
        seat: SeatId,
        client: u64,
        pid: Option<i32>,
        error: Option<ProtocolError>,
    ) {
        if let Some(marks) = &mut self.0 {
            marks.clients.push(Event::ClientLeft {
                seat,
                client,
                pid,
                error,
            });
        }
    }

    /// `seat`'s socket refused `refused` connections since it last told some.
    pub(crate) fn client_refused(&mut self, seat: SeatId, refused: u64) {
        if let Some(marks) = &mut self.0 {
            marks.clients.push(Event::ClientRefused {
                seat,
                bound: MAX_CLIENTS_PER_SEAT,
                refused,
            });
        }
    }

    /// How many windows the builds read since watching began, each window counted once a build
    /// that read it: what the tests check a build's cost by.
    pub(crate) fn windows_described(&self) -> u64 {
        self.0.as_ref().map_or(0, |marks| marks.described)
    }
}

/// From now on, marks what events tell, and marks every window and output there is now, and the
/// focus, so that the first build tells them. Watching again changes nothing.
pub(crate) fn watch(state: &mut State) {
    if state.events.0.is_some() {
        return;
    }
    let mut marks = Marks {
        focus: true,
        ..Marks::default()
    };
    for &window in state.windows.keys() {
        if let Some(seat) = state.layout.seat_of_window(window) {
            marks.windows.entry(window).or_default().opened = Some(seat);
        }
    }
    marks
        .outputs
        .extend(state.outputs.iter().map(|output| output.id));
    state.events.0 = Some(marks);
}

/// The windows whose workspace `command` may change, read before it runs, which no arrangement
/// sees when they are hidden before and after: the window a move names or the seat focuses, and
/// the windows of a workspace that loses its name. None while nobody watches, so that a command
/// reads nothing for the events then.
pub(crate) fn moved_by(state: &State, seat: SeatId, command: &Command) -> Vec<WindowId> {
    if !state.events.watching() {
        return Vec::new();
    }
    match command {
        Command::MoveWindowToWorkspace { window, .. }
        | Command::MoveWindowToSpecial { window, .. } => window
            .or_else(|| state.layout.focused(seat))
            .into_iter()
            .collect(),
        Command::MoveFocused(_) => state.layout.focused(seat).into_iter().collect(),
        Command::RemoveWorkspace(name) => state.layout.named_workspace_windows(seat, name),
        _ => Vec::new(),
    }
}

/// The events since the last build, in `yuna_ipc::Event`'s order; none while nobody watches.
pub(crate) fn take(state: &mut State) -> Vec<Event> {
    let Some(mut marks) = state.events.0.take() else {
        return Vec::new();
    };
    let mut events = Vec::new();
    marks.build_outputs(state, &mut events);
    marks.build_windows(state, &mut events);
    if std::mem::take(&mut marks.focus) {
        marks.build_focus(state, &mut events);
    }
    events.append(&mut marks.clients);
    state.events.0 = Some(marks);
    events
}

impl Marks {
    /// The outputs marked: added and changed in the order they were created, then removed, by
    /// id.
    fn build_outputs(&mut self, state: &State, events: &mut Vec<Event>) {
        if self.outputs.is_empty() {
            return;
        }
        for output in &state.outputs {
            if !self.outputs.contains(&output.id) {
                continue;
            }
            let now = output.description();
            let (seat, id) = (now.seat, now.id);
            let event = match self.told.outputs.get(&id) {
                None => Event::OutputAdded { seat, output: id },
                Some(told) if *told != now => Event::OutputChanged { seat, output: id },
                Some(_) => continue,
            };
            events.push(event);
            self.told.outputs.insert(id, now);
        }
        let mut removed: Vec<OutputId> = self
            .outputs
            .iter()
            .copied()
            .filter(|&id| state.output_index(id).is_none())
            .collect();
        removed.sort_unstable();
        for output in removed {
            if let Some(told) = self.told.outputs.remove(&output) {
                events.push(Event::OutputRemoved {
                    seat: told.seat,
                    output,
                });
            }
        }
        self.outputs.clear();
    }

    /// The windows marked, by id: opened, changed, placed, unresponsive, and closed for one
    /// opened since the last build; then the windows closed that an earlier build told, by id.
    /// A place waits while an arrangement is pending, its mark kept.
    fn build_windows(&mut self, state: &State, events: &mut Vec<Event>) {
        if self.windows.is_empty() {
            return;
        }
        let arranged = !state.layout_dirty;
        self.order.clear();
        self.order.extend(self.windows.keys().copied());
        self.order.sort_unstable();
        // A window new to the readers has its place built too.
        let places_needed = arranged
            && self.windows.iter().any(|(window, mark)| {
                !mark.closed && (mark.place || !self.told.windows.contains_key(window))
            });
        if places_needed {
            self.read_places(state);
        }
        let mut closed = Vec::new();
        for index in 0..self.order.len() {
            let window = self.order[index];
            let Some(mark) = self.windows.remove(&window) else {
                continue;
            };
            if mark.closed {
                self.build_closed(window, mark, &mut closed, events);
                continue;
            }
            let toplevel = state
                .windows
                .get(&window)
                .and_then(|surface| state.toplevels.get(surface));
            let Some(toplevel) = toplevel else {
                continue;
            };
            if self.build_open(state, window, toplevel, mark, arranged, events) {
                self.windows.entry(window).or_default().place = true;
            }
        }
        for (seat, window) in closed {
            events.push(Event::WindowClosed { seat, window });
        }
        self.places.clear();
    }

    /// Each window marked that an output arranges, with its place there.
    fn read_places(&mut self, state: &State) {
        self.places.clear();
        for output in &state.outputs {
            for placed in &output.placed {
                if self.windows.contains_key(&placed.window) {
                    self.places
                        .insert(placed.window, Arranged::of(output.id, placed));
                }
            }
        }
    }

    /// A window marked closed: told closed after the windows when a build told it opened, else
    /// opened and closed now, from what its close read.
    fn build_closed(
        &mut self,
        window: WindowId,
        mark: WindowMark,
        closed: &mut Vec<(SeatId, WindowId)>,
        events: &mut Vec<Event>,
    ) {
        if let Some(told) = self.told.windows.remove(&window) {
            closed.push((told.seat, window));
            return;
        }
        let (Some(seat), Some(facts)) = (mark.opened, mark.facts) else {
            return;
        };
        events.push(Event::WindowOpened {
            seat,
            window,
            app_id: facts.app_id,
            title: facts.title,
            pid: facts.pid,
            shell: facts.shell,
        });
        events.push(Event::WindowClosed { seat, window });
    }

    /// An open window marked: opened, changed, placed and unresponsive, each told when it differs
    /// from what was told. Returns whether its place waits for the arrangement pending.
    fn build_open(
        &mut self,
        state: &State,
        window: WindowId,
        toplevel: &Toplevel,
        mut mark: WindowMark,
        arranged: bool,
        events: &mut Vec<Event>,
    ) -> bool {
        self.described += 1;
        let told = match self.told.windows.entry(window) {
            Entry::Occupied(told) => told.into_mut(),
            Entry::Vacant(vacant) => {
                let Some(seat) = mark.opened.or_else(|| state.layout.seat_of_window(window)) else {
                    return false;
                };
                let facts = state.window_facts(toplevel);
                let (title, app_id) = (
                    bounded(facts.title.as_deref()),
                    bounded(facts.app_id.as_deref()),
                );
                events.push(Event::WindowOpened {
                    seat,
                    window,
                    app_id: app_id.clone(),
                    title: title.clone(),
                    pid: facts.pid,
                    shell: facts.shell,
                });
                // Every fact of a window new to the readers is built: its texts were just told.
                mark.texts = false;
                mark.place = true;
                mark.answers = true;
                vacant.insert(ToldWindow {
                    seat,
                    title,
                    app_id,
                    unresponsive: false,
                    placement: Placement::default(),
                })
            }
        };
        let seat = told.seat;
        if mark.texts {
            let facts = state.window_facts(toplevel);
            // Compared as told, cut, without a copy.
            let same = |told: &Option<String>, now: &Option<String>| {
                told.as_deref() == now.as_deref().map(cut)
            };
            if !same(&told.title, &facts.title) || !same(&told.app_id, &facts.app_id) {
                told.title = bounded(facts.title.as_deref());
                told.app_id = bounded(facts.app_id.as_deref());
                events.push(Event::WindowChanged {
                    seat,
                    window,
                    title: told.title.clone(),
                    app_id: told.app_id.clone(),
                });
            }
        }
        let waits = mark.place && !arranged;
        if mark.place && arranged {
            let placement = placement(state, window, toplevel, self.places.get(&window));
            if placement != told.placement {
                events.push(Event::WindowPlaced {
                    seat,
                    window,
                    output: placement.output,
                    workspace: placement.workspace.clone(),
                    place: placement.place,
                });
                told.placement = placement;
            }
        }
        if mark.answers {
            let unresponsive = toplevel
                .x11()
                .is_some_and(|x11| state.x11_unresponsive(x11));
            if unresponsive && !told.unresponsive {
                events.push(Event::WindowUnresponsive { seat, window });
            }
            told.unresponsive = unresponsive;
        }
        waits
    }

    /// Each seat's focus, by seat, told where it moved since the last build: a seat's or a
    /// seat's gone. A window the layout holds that Yuna does not show, a test seam's, is no
    /// focus to tell.
    fn build_focus(&mut self, state: &State, events: &mut Vec<Event>) {
        let mut seats = state.seat_ids();
        seats.extend(self.told.focus.keys().copied());
        seats.sort_unstable();
        seats.dedup();
        for seat in seats {
            let now = state
                .layout
                .focused(seat)
                .filter(|window| state.windows.contains_key(window));
            if now == self.told.focus.get(&seat).copied() {
                continue;
            }
            events.push(Event::Focus { seat, window: now });
            match now {
                Some(window) => self.told.focus.insert(seat, window),
                None => self.told.focus.remove(&seat),
            };
        }
    }
}

/// Where `window`, of `toplevel`, rests now, as its description gives it: the output that shows
/// it, its place in that output's arrangement, and its workspace's name, from `arranged`, its
/// place in an output's arrangement, when it has one, else from the layout's walk.
fn placement(
    state: &State,
    window: WindowId,
    toplevel: &Toplevel,
    arranged: Option<&Arranged>,
) -> Placement {
    let output = toplevel.shown_on;
    let place = arranged
        .filter(|arranged| Some(arranged.output) == output)
        .map(|arranged| arranged.rect);
    let workspace = match arranged {
        Some(arranged) => bounded(state.layout.workspace_name(arranged.workspace)),
        None => state
            .layout
            .window(window)
            .and_then(|info| bounded(info.workspace.as_deref())),
    };
    Placement {
        output,
        workspace,
        place,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_long_text_is_cut_at_a_characters_boundary() {
        // 900 bytes of a character of 3: 512 falls inside the 171st.
        let long = "€".repeat(300);
        assert_eq!(cut(&long).len(), 510);
        assert!(long.starts_with(cut(&long)));
        assert_eq!(cut("short"), "short");
        assert_eq!(cut(""), "");
        let exact = "a".repeat(MAX_EVENT_TEXT + 1);
        assert_eq!(cut(&exact).len(), MAX_EVENT_TEXT);
    }

    #[test]
    fn nothing_is_marked_or_built_unwatched() {
        let mut marks = EventMarks::default();
        marks.window_opened(WindowId(1), SeatId(2));
        marks.window_texts_changed(WindowId(1));
        marks.output_changed(OutputId(1));
        marks.focus_may_have_moved();
        marks.client_refused(SeatId(2), 1);
        assert!(marks.0.is_none());
        assert!(!marks.awaits_opening(WindowId(1)));
        assert_eq!(marks.windows_described(), 0);
    }
}
