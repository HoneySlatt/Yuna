//! The event stream against what the compositor describes (ADR-0153), with proptest: the
//! comparison of descriptions that MCP made before M2.4c, kept as a test's oracle.
//!
//! Random sequences of steps run on a manual clock, with two agent seats, each with an output
//! beside the owner's, and three test clients, two on the first seat and one on the second:
//! windows opened and closed, some opened and closed within one round; titles and application
//! ids set, long ones past the events' cut of 512 bytes included, in characters of one byte and
//! of three, and a title set twice in one round; commands of either seat (focus, workspaces shown,
//! added, removed and moved to, long names past the cut among them, special workspaces,
//! fullscreen, floating, tiling); outputs added, configured and removed; clients leaving; and
//! rounds and the clock's advances between them.
//!
//! After every step, the events taken (`Compositor::take_events`) are folded into a state: each
//! window with its facts, each output, each seat's focus. It must equal what
//! `Compositor::windows` and `Compositor::output_descriptions` describe, the texts cut as events
//! cut them, and the places compared once no arrangement is pending. Each event must change what
//! it folds into, and a window is told at most once a step for each kind: a fact told twice, or
//! changed and changed back and told, fails. An output is told changed exactly when its
//! description changed since the last step. A window opened and closed within one round leaves
//! nothing to compare: its step must tell it opened, then closed.
//!
//! Two tests share the cases: animations off, as both headless modes default, and on. A failure
//! shrinks to the shortest sequence that still fails, printed as its steps; proptest keeps its
//! seed in `event_stream.proptest-regressions`, beside this file. Each test prints its cases, the
//! time they took and the events they told, by kind; `PROPTEST_CASES=<n>` runs `n` in each.

use std::cell::RefCell;
use std::collections::{BTreeMap, HashSet};
use std::time::{Duration, Instant};

use proptest::prelude::*;
use proptest::test_runner::{FileFailurePersistence, TestRunner, contextualize_config};
use yuna::{ClockMode, Compositor, Config, OutputDescription, OutputMode};
use yuna_ipc::{Caller, Command, Event, MAX_EVENT_TEXT};
use yuna_motion::Settings;
use yuna_types::{Direction, OutputId, Rect, SeatId, Tiling, WindowShell};

use crate::RED;
use crate::client::Client;

/// The cases of each of the two tests.
const CASES_PER_TEST: u32 = 128;
/// The longest sequence of steps.
const MAX_STEPS: usize = 40;
/// The clients' slots: the first two on the first agent seat, the third on the second.
const SLOTS: usize = 3;
const SLOT_SEATS: [usize; SLOTS] = [0, 0, 1];
/// The size of each window's buffer, in pixels.
const BUFFER: (i32, i32) = (320, 200);

/// Whether the compositor's animations are on: the same cases run both ways.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Animations {
    Off,
    On,
}

/// A title or an application id a step sets: short, or long, `len` copies of `a`, or of `€`,
/// three bytes, when `wide`, then `last`, which lies past the events' cut or not.
#[derive(Debug, Clone, Copy)]
enum Text {
    Short(u8),
    Long { wide: bool, len: u16, last: char },
}

impl Text {
    fn render(self) -> String {
        match self {
            Text::Short(n) => format!("text {n}"),
            Text::Long { wide, len, last } => {
                let fill = if wide { '€' } else { 'a' };
                let mut text: String = std::iter::repeat_n(fill, usize::from(len)).collect();
                text.push(last);
                text
            }
        }
    }
}

/// A command of a seat, its names and windows picked by index.
#[derive(Debug, Clone)]
enum Order {
    /// The seat's window at this index, among those `windows` lists, modulo their count.
    FocusWindow(usize),
    Focus(Direction),
    /// The workspace [`workspace`] names by this index.
    FocusWorkspace(usize),
    FocusWorkspaceRelative(Direction),
    MoveFocused(Direction),
    AddWorkspace(usize),
    RemoveWorkspace(usize),
    MoveToWorkspace(usize),
    ToggleSpecial(usize),
    MoveToSpecial(usize),
    Fullscreen(bool),
    Floating(bool),
    /// Dwindle when true, scrolling columns otherwise.
    Dwindle(bool),
}

/// An output's mode, among a few, by index.
#[derive(Debug, Clone, Copy)]
struct Mode(usize);

impl Mode {
    fn output_mode(self) -> OutputMode {
        let (width, height, rate, scale) = [
            (640, 360, 60.0, 1.0),
            (800, 600, 60.0, 1.0),
            (640, 360, 60.0, 2.0),
            (1280, 720, 120.0, 1.5),
        ][self.0 % 4];
        OutputMode::new(width, height, rate, scale).expect("a valid mode")
    }
}

/// One step. Indices of windows and outputs are taken modulo the count of what they name; a
/// step that names nothing, there being none, does nothing.
#[derive(Debug, Clone)]
enum Step {
    /// A window of the slot's client opens, with these texts set before it maps.
    Open {
        slot: usize,
        app_id: Option<Text>,
        title: Option<Text>,
    },
    /// A window maps and closes within one round.
    OpenAndClose {
        slot: usize,
        title: Option<Text>,
    },
    /// A window of the slot's closes, as a client closes it.
    Close {
        slot: usize,
        window: usize,
    },
    /// A window of the slot's commits without a buffer: it closes, its toplevel kept.
    Unmap {
        slot: usize,
        window: usize,
    },
    SetTitle {
        slot: usize,
        window: usize,
        title: Text,
    },
    SetAppId {
        slot: usize,
        window: usize,
        app_id: Text,
    },
    /// A window's title set twice within one round: the build tells the last alone, and
    /// nothing when it is the title the window had.
    SetTitleTwice {
        slot: usize,
        window: usize,
        first: Text,
        last: Text,
    },
    Command {
        seat: usize,
        order: Order,
    },
    AddOutput {
        seat: usize,
        mode: Mode,
    },
    ConfigureOutput {
        output: usize,
        mode: Mode,
    },
    RemoveOutput {
        output: usize,
    },
    /// The slot's client leaves, its windows with it.
    Disconnect {
        slot: usize,
    },
    /// A round of the compositor, the clock still.
    Dispatch,
    /// The clock moves, by milliseconds.
    Advance(u64),
}

/// The workspace names the commands use: short ones, and two past the events' cut, of one byte
/// and of two a character.
fn workspace(index: usize) -> String {
    match index % 4 {
        0 => "one".to_owned(),
        1 => "two".to_owned(),
        2 => "w".repeat(600),
        _ => "é".repeat(300),
    }
}

fn text() -> impl Strategy<Value = Text> {
    prop_oneof![
        3 => (0_u8..4).prop_map(Text::Short),
        1 => (any::<bool>(), 150_u16..700, prop::sample::select(vec!['!', 'z']))
            .prop_map(|(wide, len, last)| Text::Long { wide, len, last }),
    ]
}

fn direction() -> impl Strategy<Value = Direction> {
    prop::sample::select(vec![
        Direction::Left,
        Direction::Right,
        Direction::Up,
        Direction::Down,
    ])
}

fn order() -> impl Strategy<Value = Order> {
    let name = 0_usize..4;
    prop_oneof![
        3 => any::<usize>().prop_map(Order::FocusWindow),
        2 => direction().prop_map(Order::Focus),
        2 => name.clone().prop_map(Order::FocusWorkspace),
        1 => direction().prop_map(Order::FocusWorkspaceRelative),
        1 => direction().prop_map(Order::MoveFocused),
        1 => name.clone().prop_map(Order::AddWorkspace),
        2 => name.clone().prop_map(Order::RemoveWorkspace),
        2 => name.clone().prop_map(Order::MoveToWorkspace),
        1 => name.clone().prop_map(Order::ToggleSpecial),
        1 => name.prop_map(Order::MoveToSpecial),
        1 => any::<bool>().prop_map(Order::Fullscreen),
        1 => any::<bool>().prop_map(Order::Floating),
        1 => any::<bool>().prop_map(Order::Dwindle),
    ]
}

fn step() -> impl Strategy<Value = Step> {
    let slot = 0..SLOTS;
    let window = any::<usize>();
    prop_oneof![
        5 => (slot.clone(), prop::option::of(text()), prop::option::of(text()))
            .prop_map(|(slot, app_id, title)| Step::Open { slot, app_id, title }),
        2 => (slot.clone(), prop::option::of(text()))
            .prop_map(|(slot, title)| Step::OpenAndClose { slot, title }),
        2 => (slot.clone(), window)
            .prop_map(|(slot, window)| Step::Close { slot, window }),
        1 => (slot.clone(), window)
            .prop_map(|(slot, window)| Step::Unmap { slot, window }),
        3 => (slot.clone(), window, text())
            .prop_map(|(slot, window, title)| Step::SetTitle { slot, window, title }),
        1 => (slot.clone(), window, text())
            .prop_map(|(slot, window, app_id)| Step::SetAppId { slot, window, app_id }),
        1 => (slot.clone(), window, text(), text()).prop_map(|(slot, window, first, last)| {
            Step::SetTitleTwice { slot, window, first, last }
        }),
        6 => (0_usize..2, order()).prop_map(|(seat, order)| Step::Command { seat, order }),
        1 => (0_usize..2, 0_usize..4)
            .prop_map(|(seat, mode)| Step::AddOutput { seat, mode: Mode(mode) }),
        1 => (any::<usize>(), 0_usize..4)
            .prop_map(|(output, mode)| Step::ConfigureOutput { output, mode: Mode(mode) }),
        1 => any::<usize>().prop_map(|output| Step::RemoveOutput { output }),
        1 => slot.prop_map(|slot| Step::Disconnect { slot }),
        1 => Just(Step::Dispatch),
        3 => (0_u64..60).prop_map(Step::Advance),
    ]
}

/// `text` as an event gives it: its first [`MAX_EVENT_TEXT`] bytes at most, in whole
/// characters.
fn cut(text: &str) -> String {
    let end = text
        .char_indices()
        .map(|(at, character)| at + character.len_utf8())
        .take_while(|&end| end <= MAX_EVENT_TEXT)
        .last()
        .unwrap_or(0);
    text[..end].to_owned()
}

fn cut_text(text: &Option<String>) -> Option<String> {
    text.as_deref().map(cut)
}

/// Fails for a text an event gives past [`MAX_EVENT_TEXT`].
fn check_cut(text: &Option<String>) -> Result<(), String> {
    match text {
        Some(text) if text.len() > MAX_EVENT_TEXT => {
            Err(format!("an event gives {} bytes of text", text.len()))
        }
        _ => Ok(()),
    }
}

/// The index `index` names among `count`, modulo; none of none.
fn pick(index: usize, count: usize) -> Option<usize> {
    (count > 0).then(|| index % count)
}

/// An event's kind, for the tally and the rule of one event a window and kind a build.
fn kind(event: &Event) -> &'static str {
    match event {
        Event::WindowOpened { .. } => "window_opened",
        Event::WindowChanged { .. } => "window_changed",
        Event::WindowPlaced { .. } => "window_placed",
        Event::WindowUnresponsive { .. } => "window_unresponsive",
        Event::WindowClosed { .. } => "window_closed",
        Event::Focus { .. } => "focus",
        Event::OutputAdded { .. } => "output_added",
        Event::OutputChanged { .. } => "output_changed",
        Event::OutputRemoved { .. } => "output_removed",
        Event::ClientLeft { .. } => "client_left",
        Event::ClientRefused { .. } => "client_refused",
        _ => "unknown",
    }
}

/// A window as the events told it.
#[derive(Debug, Clone, PartialEq)]
struct ToldWindow {
    seat: SeatId,
    app_id: Option<String>,
    title: Option<String>,
    pid: Option<i32>,
    shell: WindowShell,
    output: Option<OutputId>,
    workspace: Option<String>,
    place: Option<Rect>,
    unresponsive: bool,
}

/// What the events told, folded.
#[derive(Debug, Default)]
struct Folded {
    windows: BTreeMap<u64, ToldWindow>,
    /// Each output's seat.
    outputs: BTreeMap<u64, SeatId>,
    focus: BTreeMap<SeatId, u64>,
}

impl Folded {
    /// The window `window` an event names, which must have been told opened.
    fn window(&mut self, window: u64, event: &Event) -> Result<&mut ToldWindow, String> {
        self.windows
            .get_mut(&window)
            .ok_or_else(|| format!("{event:?} names a window never told opened"))
    }

    /// Folds one build's events in; `before` and `now` are the outputs described at the last
    /// build and at this one. Fails for an event that changes nothing, a window told twice in
    /// a kind, and an output told changed exactly when it did not change.
    fn fold(
        &mut self,
        events: &[Event],
        before: &[OutputDescription],
        now: &[OutputDescription],
    ) -> Result<(), String> {
        let mut once = HashSet::new();
        let mut changed_outputs = Vec::new();
        for event in events {
            let window = match event {
                Event::WindowOpened { window, .. }
                | Event::WindowChanged { window, .. }
                | Event::WindowPlaced { window, .. }
                | Event::WindowUnresponsive { window, .. }
                | Event::WindowClosed { window, .. } => Some(window.0),
                _ => None,
            };
            if let Some(window) = window
                && !once.insert((window, kind(event)))
            {
                return Err(format!("window {window} told {} twice", kind(event)));
            }
            self.fold_one(event, &mut changed_outputs)?;
        }
        for output in now {
            let Some(old) = before.iter().find(|old| old.id == output.id) else {
                continue;
            };
            let told = changed_outputs.contains(&output.id.0);
            if (old != output) != told {
                return Err(format!(
                    "output {} was {old:?}, is {output:?}, and was told changed: {told}",
                    output.id.0
                ));
            }
        }
        Ok(())
    }

    fn fold_one(&mut self, event: &Event, changed_outputs: &mut Vec<u64>) -> Result<(), String> {
        let unchanged = || Err(format!("{event:?} changes nothing"));
        match event {
            Event::WindowOpened {
                seat,
                window,
                app_id,
                title,
                pid,
                shell,
            } => {
                check_cut(app_id)?;
                check_cut(title)?;
                let opened = ToldWindow {
                    seat: *seat,
                    app_id: app_id.clone(),
                    title: title.clone(),
                    pid: *pid,
                    shell: *shell,
                    output: None,
                    workspace: None,
                    place: None,
                    unresponsive: false,
                };
                if self.windows.insert(window.0, opened).is_some() {
                    return Err(format!("window {} told opened twice", window.0));
                }
            }
            Event::WindowChanged {
                window,
                title,
                app_id,
                ..
            } => {
                check_cut(title)?;
                check_cut(app_id)?;
                let told = self.window(window.0, event)?;
                if (&told.title, &told.app_id) == (title, app_id) {
                    return unchanged();
                }
                told.title = title.clone();
                told.app_id = app_id.clone();
            }
            Event::WindowPlaced {
                window,
                output,
                workspace,
                place,
                ..
            } => {
                check_cut(workspace)?;
                let told = self.window(window.0, event)?;
                if (told.output, &told.workspace, told.place) == (*output, workspace, *place) {
                    return unchanged();
                }
                told.output = *output;
                told.workspace = workspace.clone();
                told.place = *place;
            }
            Event::WindowUnresponsive { window, .. } => {
                let told = self.window(window.0, event)?;
                if told.unresponsive {
                    return unchanged();
                }
                told.unresponsive = true;
            }
            Event::WindowClosed { window, .. } => {
                if self.windows.remove(&window.0).is_none() {
                    return Err(format!("{event:?} names a window never told opened"));
                }
            }
            Event::Focus { seat, window } => {
                let window = window.map(|window| window.0);
                if self.focus.get(seat).copied() == window {
                    return unchanged();
                }
                match window {
                    Some(window) => self.focus.insert(*seat, window),
                    None => self.focus.remove(seat),
                };
            }
            Event::OutputAdded { seat, output } => {
                if self.outputs.insert(output.0, *seat).is_some() {
                    return Err(format!("output {} told added twice", output.0));
                }
            }
            Event::OutputChanged { output, .. } => {
                if !self.outputs.contains_key(&output.0) {
                    return Err(format!("{event:?} names an output never told added"));
                }
                changed_outputs.push(output.0);
            }
            Event::OutputRemoved { output, .. } => {
                if self.outputs.remove(&output.0).is_none() {
                    return Err(format!("{event:?} names an output never told added"));
                }
            }
            Event::ClientLeft { .. } | Event::ClientRefused { .. } => {}
            _ => return Err(format!("{event:?}: a kind the oracle does not know")),
        }
        Ok(())
    }

    /// Compares what was told with what `compositor` describes now: the windows, their texts
    /// cut, their places once no arrangement is pending; each seat's focus; the outputs.
    fn compare(&self, compositor: &Compositor) -> Result<(), String> {
        let windows = compositor.windows();
        let pending = compositor.arrangement_pending();
        let described: Vec<u64> = windows.iter().map(|window| window.id.0).collect();
        let told: Vec<u64> = self.windows.keys().copied().collect();
        if described != told {
            return Err(format!("windows {described:?} described, {told:?} told"));
        }
        for window in &windows {
            let told = &self.windows[&window.id.0];
            let described = ToldWindow {
                seat: window.seat,
                app_id: cut_text(&window.app_id),
                title: cut_text(&window.title),
                pid: window.pid,
                shell: window.shell,
                output: if pending { told.output } else { window.output },
                workspace: if pending {
                    told.workspace.clone()
                } else {
                    cut_text(&window.workspace)
                },
                place: if pending { told.place } else { window.place },
                unresponsive: window.unresponsive,
            };
            if *told != described {
                return Err(format!(
                    "window {}: told {told:?}, described {described:?}",
                    window.id.0
                ));
            }
        }
        for seat in compositor.seats() {
            let described = windows
                .iter()
                .find(|window| window.seat == seat && window.focused)
                .map(|window| window.id.0);
            let told = self.focus.get(&seat).copied();
            if described != told {
                return Err(format!(
                    "seat {} focuses {described:?}, told {told:?}",
                    seat.0
                ));
            }
        }
        let outputs: BTreeMap<u64, SeatId> = compositor
            .output_descriptions()
            .iter()
            .map(|output| (output.id.0, output.seat))
            .collect();
        if outputs != self.outputs {
            return Err(format!(
                "outputs {outputs:?} described, {:?} told",
                self.outputs
            ));
        }
        Ok(())
    }
}

/// A case: the compositor, its seats, the clients of the slots with their windows open, and
/// what the events told.
struct Run {
    compositor: Compositor,
    seats: [SeatId; 2],
    clients: [Option<Client>; SLOTS],
    /// Each slot's windows open, by their index in its client.
    open: [Vec<usize>; SLOTS],
    folded: Folded,
    /// The outputs described at the last build.
    outputs: Vec<OutputDescription>,
    /// The events told, by kind.
    tally: BTreeMap<&'static str, u64>,
}

impl Run {
    /// A compositor watched from its start: the owner's output, then two agent seats with an
    /// output each.
    fn new(animations: Animations) -> Run {
        let mode = Mode(0).output_mode();
        let mut compositor = Compositor::new(Config {
            outputs: vec![mode],
            clock: ClockMode::Manual,
            listen: false,
            animations: Settings {
                off: animations == Animations::Off,
                ..Settings::default()
            },
            ..Config::default()
        })
        .expect("the compositor starts");
        compositor.watch_events();
        let seats = [0, 1].map(|_| compositor.add_seat().expect("an agent seat"));
        for seat in seats {
            compositor.add_output(seat, mode).expect("an output");
        }
        Run {
            compositor,
            seats,
            clients: Default::default(),
            open: Default::default(),
            folded: Folded::default(),
            outputs: Vec::new(),
            tally: BTreeMap::new(),
        }
    }

    /// The slot's client, connected now if it has none.
    fn client(&mut self, slot: usize) -> &mut Client {
        let seat = self.seats[SLOT_SEATS[slot]];
        let compositor = &mut self.compositor;
        self.clients[slot].get_or_insert_with(|| Client::connect_to(compositor, seat))
    }

    /// Forgets the slot's client, whose connection ended, and its windows.
    fn drop_client(&mut self, slot: usize) {
        self.clients[slot] = None;
        self.open[slot].clear();
    }

    /// The window of the slot's that `index` names, by its index in the client, taken out of
    /// those open when `closing`.
    fn window(&mut self, slot: usize, index: usize, closing: bool) -> Option<usize> {
        let open = &mut self.open[slot];
        let at = pick(index, open.len())?;
        Some(if closing { open.remove(at) } else { open[at] })
    }

    /// Opens a window of the slot's client, with its texts, closed in the same round when
    /// `close`.
    fn open_window(&mut self, slot: usize, app_id: Option<Text>, title: Option<Text>, close: bool) {
        let client = self.client(slot);
        let window = client.create_window();
        if let Some(app_id) = app_id {
            client.set_app_id(window, &app_id.render());
        }
        if let Some(title) = title {
            client.set_title(window, &title.render());
        }
        client.commit(window);
        let compositor = &mut self.compositor;
        let Some(client) = self.clients[slot].as_mut() else {
            return;
        };
        if client.try_roundtrip(compositor).is_err() {
            self.drop_client(slot);
            return;
        }
        client.ack(window);
        client.attach(window, BUFFER.0, BUFFER.1, RED);
        client.commit(window);
        if close {
            client.close(window);
        } else {
            self.open[slot].push(window);
        }
    }

    /// The command `order` names for `seat`.
    fn command(&self, seat: SeatId, order: &Order) -> Option<Command> {
        Some(match order {
            Order::FocusWindow(index) => {
                let windows: Vec<_> = self
                    .compositor
                    .windows()
                    .into_iter()
                    .filter(|window| window.seat == seat)
                    .collect();
                Command::FocusWindow(windows[pick(*index, windows.len())?].id)
            }
            Order::Focus(direction) => Command::Focus(*direction),
            Order::FocusWorkspace(name) => Command::FocusWorkspace(workspace(*name)),
            Order::FocusWorkspaceRelative(direction) => Command::FocusWorkspaceRelative(*direction),
            Order::MoveFocused(direction) => Command::MoveFocused(*direction),
            Order::AddWorkspace(name) => Command::AddWorkspace {
                name: workspace(*name),
                output: None,
            },
            Order::RemoveWorkspace(name) => Command::RemoveWorkspace(workspace(*name)),
            Order::MoveToWorkspace(name) => Command::MoveWindowToWorkspace {
                window: None,
                workspace: workspace(*name),
            },
            Order::ToggleSpecial(name) => Command::ToggleSpecial(workspace(*name)),
            Order::MoveToSpecial(name) => Command::MoveWindowToSpecial {
                window: None,
                special: workspace(*name),
            },
            Order::Fullscreen(on) => Command::SetFullscreen {
                window: None,
                on: *on,
            },
            Order::Floating(on) => Command::SetFloating {
                window: None,
                on: *on,
            },
            Order::Dwindle(on) => Command::SetTiling(if *on {
                Tiling::Dwindle
            } else {
                Tiling::Scrolling
            }),
        })
    }

    /// Does `step`; what Yuna refuses is allowed, and changes nothing.
    fn apply(&mut self, step: &Step) {
        match step {
            Step::Open {
                slot,
                app_id,
                title,
            } => self.open_window(*slot, *app_id, *title, false),
            Step::OpenAndClose { slot, title } => self.open_window(*slot, None, *title, true),
            Step::Close { slot, window } => {
                if let Some(window) = self.window(*slot, *window, true) {
                    self.client(*slot).close(window);
                }
            }
            Step::Unmap { slot, window } => {
                if let Some(window) = self.window(*slot, *window, true) {
                    self.client(*slot).unmap(window);
                }
            }
            Step::SetTitle {
                slot,
                window,
                title,
            } => {
                if let Some(window) = self.window(*slot, *window, false) {
                    self.client(*slot).set_title(window, &title.render());
                }
            }
            Step::SetAppId {
                slot,
                window,
                app_id,
            } => {
                if let Some(window) = self.window(*slot, *window, false) {
                    self.client(*slot).set_app_id(window, &app_id.render());
                }
            }
            Step::SetTitleTwice {
                slot,
                window,
                first,
                last,
            } => {
                if let Some(window) = self.window(*slot, *window, false) {
                    let client = self.client(*slot);
                    client.set_title(window, &first.render());
                    client.set_title(window, &last.render());
                }
            }
            Step::Command { seat, order } => {
                let seat = self.seats[*seat];
                if let Some(command) = self.command(seat, order) {
                    let _ = self.compositor.command(Caller::on_seat(seat), command);
                }
            }
            Step::AddOutput { seat, mode } => {
                let _ = self
                    .compositor
                    .add_output(self.seats[*seat], mode.output_mode());
            }
            Step::ConfigureOutput { output, mode } => {
                let outputs = self.compositor.outputs();
                if let Some(at) = pick(*output, outputs.len()) {
                    let _ = self
                        .compositor
                        .configure_output(outputs[at], mode.output_mode());
                }
            }
            Step::RemoveOutput { output } => {
                let outputs = self.compositor.outputs();
                if let Some(at) = pick(*output, outputs.len()) {
                    let _ = self.compositor.remove_output(outputs[at]);
                }
            }
            Step::Disconnect { slot } => self.drop_client(*slot),
            Step::Dispatch => self
                .compositor
                .dispatch(Some(Duration::ZERO))
                .expect("the compositor dispatches"),
            Step::Advance(millis) => self
                .compositor
                .advance(Duration::from_millis(*millis))
                .expect("the manual clock moves"),
        }
    }

    /// Lets each client send what it wrote and read what came, forgets those whose connection
    /// ended, and runs a round, which frees a client that left.
    fn settle(&mut self) {
        for slot in 0..SLOTS {
            let compositor = &mut self.compositor;
            if let Some(client) = self.clients[slot].as_mut()
                && client.try_roundtrip(compositor).is_err()
            {
                self.drop_client(slot);
            }
        }
        self.compositor
            .dispatch(Some(Duration::ZERO))
            .expect("the compositor dispatches");
    }

    /// Takes the events, folds them in, and compares what they told with what is described.
    /// After a step that opened and closed a window in one round, they must tell a window
    /// opened, then closed.
    fn check(&mut self, opened_and_closed: bool) -> Result<(), String> {
        let events = self.compositor.take_events();
        if opened_and_closed {
            let opened = events.iter().find_map(|event| match event {
                Event::WindowOpened { window, .. } => Some(*window),
                _ => None,
            });
            let closed = opened.and_then(|opened| {
                events.iter().position(|event| {
                    matches!(event, Event::WindowClosed { window, .. } if *window == opened)
                })
            });
            if closed.is_none() {
                return Err(format!(
                    "a window opened and closed in one round is not told both: {events:?}"
                ));
            }
        }
        let now = self.compositor.output_descriptions();
        for event in &events {
            *self.tally.entry(kind(event)).or_default() += 1;
        }
        self.folded.fold(&events, &self.outputs, &now)?;
        self.outputs = now;
        self.folded.compare(&self.compositor)
    }
}

/// Runs `steps`, checking the events after the start and after each step.
fn run(
    animations: Animations,
    steps: &[Step],
    tally: &mut BTreeMap<&'static str, u64>,
) -> Result<(), TestCaseError> {
    let mut run = Run::new(animations);
    let checked = run.check(false);
    if let Err(error) = checked {
        return Err(TestCaseError::fail(format!("at the start: {error}")));
    }
    for (index, step) in steps.iter().enumerate() {
        run.apply(step);
        run.settle();
        if let Err(error) = run.check(matches!(step, Step::OpenAndClose { .. })) {
            return Err(TestCaseError::fail(format!(
                "step {index}, {step:?}: {error}"
            )));
        }
    }
    for (kind, count) in run.tally {
        *tally.entry(kind).or_default() += count;
    }
    Ok(())
}

/// Runs the cases of one test, then prints how many, how long they took and what they told.
fn run_cases(animations: Animations) {
    let mut config = contextualize_config(ProptestConfig {
        cases: CASES_PER_TEST,
        // Beside this file, as `proptest!` would set it.
        failure_persistence: Some(Box::new(FileFailurePersistence::WithSource(
            "proptest-regressions",
        ))),
        ..ProptestConfig::default()
    });
    config.source_file = Some(file!());
    let cases = config.cases;
    let tally = RefCell::new(BTreeMap::new());
    let mut runner = TestRunner::new(config);
    let started = Instant::now();
    let result = runner.run(&prop::collection::vec(step(), 1..=MAX_STEPS), |steps| {
        run(animations, &steps, &mut tally.borrow_mut())
    });
    if let Err(error) = result {
        panic!("{error}\n{runner}");
    }
    println!(
        "{cases} cases, animations {animations:?}, in {:?}: told {:?}",
        started.elapsed(),
        tally.borrow()
    );
}

#[test]
fn the_event_stream_tells_what_the_compositor_describes() {
    run_cases(Animations::Off);
}

#[test]
fn the_event_stream_tells_what_the_compositor_describes_with_animations() {
    run_cases(Animations::On);
}

/// Sequences whose marks the random cases seldom reach, written out, each run with animations
/// off and on: a workspace that loses its name, its windows shown, then hidden on it.
#[test]
fn sequences_written_out() {
    let on_first = |order| Step::Command { seat: 0, order };
    let open = Step::Open {
        slot: 0,
        app_id: None,
        title: None,
    };
    let sequences = [
        // Shown on "one", which loses its name: no arrangement moves it.
        vec![
            on_first(Order::FocusWorkspace(0)),
            open.clone(),
            Step::Advance(20),
            on_first(Order::RemoveWorkspace(0)),
            Step::Advance(20),
        ],
        // Moved to a workspace whose name passes the events' cut, hidden there; then that
        // workspace loses its name: no arrangement sees the window.
        vec![
            open.clone(),
            Step::Advance(20),
            on_first(Order::MoveToWorkspace(2)),
            Step::Advance(20),
            on_first(Order::RemoveWorkspace(2)),
            Step::Advance(20),
        ],
    ];
    for steps in sequences {
        for animations in [Animations::Off, Animations::On] {
            if let Err(error) = run(animations, &steps, &mut BTreeMap::new()) {
                panic!("{error}, animations {animations:?}");
            }
        }
    }
}
