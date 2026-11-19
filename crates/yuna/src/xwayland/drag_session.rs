//! The decisions of a seat's drags between its X11 programs and its Wayland programs, on Yuna's
//! side, in plain Rust: no Smithay, wayland-server or x11rb type, so that each rule is tested
//! alone, as the layout's are. `drags.rs` feeds a seat's [`SeatDrags`] what happens, the
//! pointer's picks and release, the X11 manager's events and the Wayland side's answers, and
//! carries out the [`Effect`]s it returns: Smithay's data device on a Wayland target, commands for
//! the seat's X11 manager, events to a Wayland source. `xdnd.rs` speaks XDND on X11's side, on the
//! manager's thread, and answers the X11 peer at once from what Yuna last told it.
//!
//! **What a drag is over** ([`Pick`]): among the seat's layout windows shown under the pointer,
//! never a menu, a tooltip or an X11 drag icon, never a hidden window: a Wayland surface, an X11
//! window, or nothing.
//!
//! **An X11 program's drag.** It starts when the manager sees an X11 client take
//! `XdndSelection` while the seat's pointer holds a button on a window of that same client of
//! that Xwayland; otherwise nothing happens. Smithay's implicit grab of the press keeps sending
//! the pointer to the X11 source, which looks for its target in X11 itself: the manager's proxy,
//! a window over the whole root, is shown while the drag is over a Wayland window or over
//! nothing, and hidden over an X11 window, so that X11's own drags work as they do without Yuna,
//! and from the drop or the drag's end on. Once it is shown or hidden, while the button is held,
//! the pointer's last motion goes again: the source may have looked for its target for that
//! motion before, and would look no more until the next. A release that reaches the source
//! before it looked again is not covered: the source decides on the target it found. The proxy
//! stays at the release, whether the source entered it or not: the source may still read the
//! window under the pointer for a motion that came before its release, in the same round too,
//! and would leave a proxy gone, or find the X11 window under it, hidden or covered. It hides at
//! the source's drop or leave, when the drag ends, at [`X11_DROP_TIMEOUT`], and once the pointer
//! comes over an X11 window, after the release too, so that it never takes an X11 window's
//! pointer: an InputOnly window, it takes X11's pointer only while the pointer is over an X11
//! window. Two races are left, Yuna's loop never waiting for the manager's thread: a source that
//! looks for its target before the manager has mapped the proxy, the move and the release in one
//! round, finds the X11 window under the pointer; and a press that reaches Xwayland before the
//! manager has unmapped the proxy, in the round of the motion over an X11 window that hid it,
//! goes to the proxy (`docs/debt.md`). A Wayland target is entered only once the source's
//! `XdndEnter` and first position reached the proxy, its types and action known, and left at its
//! `XdndLeave`; the manager tells no `XdndEnter` again with the same types. Nothing happens at
//! the release: the source decides. Its `XdndDrop` drops on the Wayland target if the target
//! accepted a type and an action, else it is refused with `XdndFinished`. The target's `finish`
//! is the source's `XdndFinished`. Without `XdndDrop` or `XdndLeave` within [`X11_DROP_TIMEOUT`]
//! of the release, the drag ends.
//!
//! **A Wayland client's drag.** Smithay's drag grab carries it among Wayland windows; over an
//! X11 window it gets no focus, and the drag speaks XDND through the manager instead, its point
//! in X11's root pixels. Released over an X11 window, the manager drops if the target's last
//! status accepted, else the source is cancelled; the source is told it was performed only once
//! the target's `XdndFinished` says so, with an action it offered (`drags.rs`). Once the grab
//! ends, the seat's pointer goes back on what is under it, at the round's end: Smithay leaves it
//! on the grab's last focus, none over an X11 window. A drag within one client, which has no
//! data, offers nothing to X11. An X11 target window gone, `XdndSelection` taken by another X11
//! client, or Xwayland gone, while the button is held, ends the drag's X11 side: the grab goes
//! on among Wayland windows, and a release over an X11 window cancels the source. Dropping on
//! X11, the source is cancelled at once. A source is cancelled once.
//!
//! **Two at most.** A seat holds the drag in the pointer's hand and one ending, whose data or
//! answer still runs: [`MAX_SESSIONS`]. A new one ends any still in the hand, and the oldest
//! ending one beyond the bound, as refused; but a Wayland drag started while another is in the
//! hand, in the same grab, leaves the first to Smithay, which ends its grab, the oldest, at once.
//! An event that names a drag no longer held is left.

use std::time::Duration;

/// How many drags a seat holds at once: the one in the pointer's hand, and one ending.
pub(crate) const MAX_SESSIONS: usize = 2;

/// How long an X11 program's drag waits, after the pointer's release, for its source's
/// `XdndDrop` or `XdndLeave`: a source sends one at once, or after its own wait for a status,
/// as long as an X11 transfer waits ([`X11_TRANSFER_TIMEOUT`](crate::X11_TRANSFER_TIMEOUT)).
pub(crate) const X11_DROP_TIMEOUT: Duration = Duration::from_secs(5);

/// How many times an X11 program's drag may enter the proxy each time the proxy is shown: a
/// toolkit enters it once, when the pointer comes over a Wayland window or over nothing, which
/// only the pointer's moves repeat. Each enter offers the Wayland window under the pointer the
/// drag again, up to 64 types of 255 bytes: past the bound, an X11 program that leaves and enters
/// again without end gets nothing more until the proxy is shown again, so that it cannot flood a
/// Wayland program with offers until Yuna cuts it off.
pub(crate) const MAX_X11_ENTERS: u8 = 2;

/// What a drop does with the data, as Wayland and XDND both name it; none for a target that
/// accepts nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum DragAction {
    #[default]
    None,
    Copy,
    Move,
    Ask,
}

/// Which drag a session is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum DragKey {
    /// An X11 program's: the `drag`-th taking of `XdndSelection` Xwayland `server`'s manager saw.
    FromX11 { server: u64, drag: u64 },
    /// A Wayland client's, numbered by Yuna.
    FromWayland(u64),
}

/// What a drag is over, among the seat's layout windows shown under the pointer.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Pick<T> {
    /// No window.
    Nothing,
    /// A Wayland window's surface under the pointer: a subsurface or a popup counts.
    Wayland(T),
    /// An X11 window of Xwayland `server`, the pointer at `point` in X11's root pixels; none
    /// when that point is beyond X11's 16-bit coordinates.
    X11 {
        server: u64,
        window: u32,
        point: Option<(i16, i16)>,
    },
}

/// The X11 window a seat's pointer holds a button on: its Xwayland and its client's bits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Pressed {
    pub server: u64,
    pub client: u32,
}

/// What a decision asks the glue to do (`drags.rs`).
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Effect<T> {
    /// Shows the manager's proxy over X11's root for X11 drag `drag`, or hides it.
    ShowProxy { server: u64, drag: u64, shown: bool },
    /// Tells the manager the Wayland target's answer, which its `XdndStatus` carries.
    Answer {
        server: u64,
        drag: u64,
        accepted: bool,
        action: DragAction,
    },
    /// Ends an X11 drag with `XdndFinished`.
    Finish {
        server: u64,
        drag: u64,
        accepted: bool,
        action: DragAction,
    },
    /// Ends an X11 drag on the manager's side with nothing more said to its source.
    Forget { server: u64, drag: u64 },
    /// The drag enters the Wayland target, which is offered its data.
    Enter { key: DragKey, target: T },
    /// The drag moves over the Wayland target it entered.
    Motion { key: DragKey, target: T },
    /// The drag leaves the Wayland target.
    Leave { key: DragKey, target: T },
    /// The drag drops on the Wayland target, then leaves it.
    Drop { key: DragKey, target: T },
    /// The pointer's last motion again, and a frame, so that the X11 source looks again.
    RepeatMotion,
    /// Wayland drag `drag` is over X11 window `window` at a root point, or over none.
    Over {
        drag: u64,
        server: u64,
        over: Option<(u32, (i16, i16))>,
    },
    /// Wayland drag `drag` was released over an X11 window: the manager drops or leaves.
    DropOnX11 { drag: u64, server: u64 },
    /// Wayland drag `drag` ends on X11's side: left if not dropped, `XdndSelection` released.
    CancelX11 { drag: u64, server: u64 },
    /// The X11 target's answer goes to the Wayland source: its `target` and its `action`.
    Status {
        drag: u64,
        accepted: bool,
        action: DragAction,
    },
    /// The Wayland source is cancelled.
    Cancel { drag: u64 },
    /// The Wayland source's drop was performed, with this action, and is finished.
    Performed { drag: u64, action: DragAction },
    /// The session ended: what the glue holds for it goes.
    Ended(DragKey),
}

/// A seat's drags, at most [`MAX_SESSIONS`], oldest first. `T` names a Wayland target: a
/// surface in Yuna, a number in the tests.
#[derive(Debug)]
pub(crate) struct SeatDrags<T> {
    sessions: Vec<Session<T>>,
}

impl<T> Default for SeatDrags<T> {
    fn default() -> SeatDrags<T> {
        SeatDrags {
            sessions: Vec::new(),
        }
    }
}

#[derive(Debug)]
enum Session<T> {
    FromX11(FromX11<T>),
    FromWayland(FromWayland),
}

/// An X11 program's drag, as Yuna's side follows it.
#[derive(Debug)]
struct FromX11<T> {
    server: u64,
    drag: u64,
    stage: X11Stage,
    /// The source's `XdndEnter` reached the proxy, and no `XdndLeave` since: its types are
    /// known, and a Wayland target may be entered.
    entered: bool,
    /// How many times the source entered the proxy since it was last shown, up to
    /// [`MAX_X11_ENTERS`].
    enters: u8,
    /// What the drag was last over.
    pick: Pick<T>,
    /// Whether the proxy is shown, as the manager was last told.
    proxy_shown: bool,
    /// The Wayland target entered.
    target: Option<T>,
    /// The target's answer, as the manager was last told.
    answer: (bool, DragAction),
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum X11Stage {
    /// The pointer holds the button.
    Dragging,
    /// Released at this time on Yuna's clock: the source's drop or leave is waited for.
    Released { at: Duration },
    /// Dropped on the Wayland target, whose `finish` is waited for.
    Dropped,
}

/// A Wayland client's drag, as far as X11 is concerned.
#[derive(Debug)]
struct FromWayland {
    drag: u64,
    /// Whether it has data: a `wl_data_source`, not a drag within one client.
    data: bool,
    stage: WaylandStage,
    x11: X11Part,
    /// The X11 window it is over, and its Xwayland, while in the hand.
    over: Option<(u64, u32)>,
    /// What the source was last told of X11's answer: none since the drag came over an X11
    /// window, when a Wayland target may have told it otherwise.
    status: Option<(bool, DragAction)>,
    /// Whether its source was cancelled: it is cancelled once.
    cancelled: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum WaylandStage {
    /// The pointer holds the button, through Smithay's grab.
    Dragging,
    /// Released over an X11 window: the manager's outcome is waited for.
    Dropping,
}

/// Whether a Wayland client's drag speaks XDND.
#[derive(Debug, Clone, Copy, PartialEq)]
enum X11Part {
    /// It has not reached an X11 window.
    Unused,
    /// It speaks to Xwayland `server`'s manager.
    Speaking { server: u64 },
    /// Its X11 side ended: it offers X11 nothing more.
    Ended,
}

impl<T> Session<T> {
    fn key(&self) -> DragKey {
        match self {
            Session::FromX11(session) => DragKey::FromX11 {
                server: session.server,
                drag: session.drag,
            },
            Session::FromWayland(session) => DragKey::FromWayland(session.drag),
        }
    }

    /// Whether the pointer holds it.
    fn in_hand(&self) -> bool {
        match self {
            Session::FromX11(session) => session.stage == X11Stage::Dragging,
            Session::FromWayland(session) => session.stage == WaylandStage::Dragging,
        }
    }
}

impl<T: Clone + PartialEq> Session<T> {
    /// Ends it as refused, for a newer drag or a closing seat: an X11 source is told so, a
    /// Wayland source cancelled.
    fn refuse(self, effects: &mut Vec<Effect<T>>) {
        let key = self.key();
        match self {
            Session::FromX11(mut session) => {
                session.leave_target(effects);
                let (server, drag) = (session.server, session.drag);
                effects.push(match session.stage {
                    X11Stage::Dropped => Effect::Finish {
                        server,
                        drag,
                        accepted: false,
                        action: DragAction::None,
                    },
                    X11Stage::Dragging | X11Stage::Released { .. } => {
                        Effect::Forget { server, drag }
                    }
                });
            }
            Session::FromWayland(mut session) => {
                if let X11Part::Speaking { server } = session.x11 {
                    effects.push(Effect::CancelX11 {
                        drag: session.drag,
                        server,
                    });
                }
                session.cancel(effects);
            }
        }
        effects.push(Effect::Ended(key));
    }
}

impl<T: Clone + PartialEq> FromX11<T> {
    /// The drag moved over `pick`: the proxy shows over anything but an X11 window, and the
    /// Wayland target follows.
    fn moved(&mut self, pick: Pick<T>, effects: &mut Vec<Effect<T>>) {
        if self.stage != X11Stage::Dragging {
            return;
        }
        let shown = !matches!(pick, Pick::X11 { .. });
        self.show_proxy(shown, effects);
        self.pick = pick;
        self.follow_pick(effects);
    }

    fn show_proxy(&mut self, shown: bool, effects: &mut Vec<Effect<T>>) {
        if shown != self.proxy_shown {
            self.proxy_shown = shown;
            if shown {
                self.enters = 0;
            }
            effects.push(Effect::ShowProxy {
                server: self.server,
                drag: self.drag,
                shown,
            });
        }
    }

    /// Enters the Wayland window picked once the source's types are known, moves over it, or
    /// leaves the one entered.
    fn follow_pick(&mut self, effects: &mut Vec<Effect<T>>) {
        let key = DragKey::FromX11 {
            server: self.server,
            drag: self.drag,
        };
        let wanted = match &self.pick {
            Pick::Wayland(target) if self.entered => Some(target.clone()),
            _ => None,
        };
        if let (Some(current), Some(wanted)) = (&self.target, &wanted)
            && current == wanted
        {
            effects.push(Effect::Motion {
                key,
                target: current.clone(),
            });
            return;
        }
        self.leave_target(effects);
        if let Some(target) = wanted {
            effects.push(Effect::Enter {
                key,
                target: target.clone(),
            });
            self.target = Some(target);
        }
    }

    /// Leaves the Wayland target, if one is entered: over nothing, the drag is not accepted.
    fn leave_target(&mut self, effects: &mut Vec<Effect<T>>) {
        let Some(target) = self.target.take() else {
            return;
        };
        effects.push(Effect::Leave {
            key: DragKey::FromX11 {
                server: self.server,
                drag: self.drag,
            },
            target,
        });
        self.tell_answer((false, DragAction::None), effects);
    }

    fn tell_answer(&mut self, answer: (bool, DragAction), effects: &mut Vec<Effect<T>>) {
        if answer != self.answer {
            self.answer = answer;
            effects.push(Effect::Answer {
                server: self.server,
                drag: self.drag,
                accepted: answer.0,
                action: answer.1,
            });
        }
    }
}

impl FromWayland {
    /// The drag moved over `pick`: over an X11 window it speaks XDND, if it has data and its
    /// X11 side has not ended, and Smithay's grab gets no focus; over a Wayland surface, the
    /// grab gets it. Returns the grab's focus.
    fn moved<T>(&mut self, pick: Pick<T>, effects: &mut Vec<Effect<T>>) -> Option<T> {
        match pick {
            Pick::X11 {
                server,
                window,
                point,
            } => {
                if !self.data || self.x11 == X11Part::Ended {
                    return None;
                }
                // Beyond X11's coordinates, no point can be said: the target keeps the last.
                let point = point?;
                if self.over.is_none() {
                    // Not accepted until the X11 window says so, whatever a Wayland target
                    // told the source before.
                    self.status = None;
                    self.tell_status((false, DragAction::None), effects);
                }
                self.x11 = X11Part::Speaking { server };
                self.over = Some((server, window));
                effects.push(Effect::Over {
                    drag: self.drag,
                    server,
                    over: Some((window, point)),
                });
                None
            }
            Pick::Wayland(target) => {
                self.leave_x11(effects);
                Some(target)
            }
            Pick::Nothing => {
                self.leave_x11(effects);
                None
            }
        }
    }

    /// Leaves the X11 window it is over, if any.
    fn leave_x11<T>(&mut self, effects: &mut Vec<Effect<T>>) {
        if let Some((server, _)) = self.over.take() {
            effects.push(Effect::Over {
                drag: self.drag,
                server,
                over: None,
            });
            self.tell_status((false, DragAction::None), effects);
        }
    }

    fn tell_status<T>(&mut self, status: (bool, DragAction), effects: &mut Vec<Effect<T>>) {
        if Some(status) != self.status {
            self.status = Some(status);
            effects.push(Effect::Status {
                drag: self.drag,
                accepted: status.0,
                action: status.1,
            });
        }
    }

    /// Cancels its source, unless it was already.
    fn cancel<T>(&mut self, effects: &mut Vec<Effect<T>>) {
        if !std::mem::replace(&mut self.cancelled, true) {
            effects.push(Effect::Cancel { drag: self.drag });
        }
    }

    /// Its X11 side ends short, while the drag is in the hand: Smithay's grab goes on among
    /// Wayland windows, offering X11 nothing more, and its release decides, a cancel over an X11
    /// window.
    fn end_x11<T>(&mut self, effects: &mut Vec<Effect<T>>) {
        self.x11 = X11Part::Ended;
        self.over = None;
        self.tell_status((false, DragAction::None), effects);
    }
}

impl<T: Clone + PartialEq> SeatDrags<T> {
    /// Whether it holds no drag: nothing to follow at the pointer's motion.
    pub(crate) fn is_empty(&self) -> bool {
        self.sessions.is_empty()
    }

    /// Whether a drag is in the pointer's hand: its motion goes to it.
    pub(crate) fn in_hand(&self) -> bool {
        self.sessions.iter().any(Session::in_hand)
    }

    /// Makes room for a new drag: those still in the hand end as refused, but a Wayland drag
    /// when the new one is a Wayland drag too, which Smithay's new grab ends itself, through its
    /// own grab's end; then the oldest ending ones, so that one at most is left ending beside
    /// the new one.
    fn make_room(&mut self, new_from_wayland: bool, effects: &mut Vec<Effect<T>>) {
        let ended_by_smithay =
            |session: &Session<T>| new_from_wayland && matches!(session, Session::FromWayland(_));
        let mut index = 0;
        while index < self.sessions.len() {
            let session = &self.sessions[index];
            if session.in_hand() && !ended_by_smithay(session) {
                self.sessions.remove(index).refuse(effects);
            } else {
                index += 1;
            }
        }
        let ending =
            |sessions: &[Session<T>]| sessions.iter().filter(|session| !session.in_hand()).count();
        while ending(&self.sessions) >= MAX_SESSIONS {
            let Some(oldest) = self.sessions.iter().position(|session| !session.in_hand()) else {
                break;
            };
            self.sessions.remove(oldest).refuse(effects);
        }
    }

    fn x11_session(&mut self, server: u64, drag: u64) -> Option<&mut FromX11<T>> {
        self.sessions.iter_mut().find_map(|session| match session {
            Session::FromX11(session) if session.server == server && session.drag == drag => {
                Some(session)
            }
            _ => None,
        })
    }

    fn wayland_session(&mut self, drag: u64) -> Option<&mut FromWayland> {
        self.sessions.iter_mut().find_map(|session| match session {
            Session::FromWayland(session) if session.drag == drag => Some(session),
            _ => None,
        })
    }

    /// Removes the session `key`, and says so.
    fn end(&mut self, key: DragKey, effects: &mut Vec<Effect<T>>) {
        self.sessions.retain(|session| session.key() != key);
        effects.push(Effect::Ended(key));
    }

    /// Xwayland `server`'s manager saw X11 client `client` take `XdndSelection`, its `drag`-th
    /// taking; `pressed` is the X11 window the seat's pointer holds a button on, if it holds
    /// one on an X11 window, and `pick` what the pointer is over. A drag starts only from a
    /// button held on a window of that same client of that Xwayland; none when it does not.
    pub(crate) fn x11_offered(
        &mut self,
        server: u64,
        drag: u64,
        client: u32,
        pressed: Option<Pressed>,
        pick: Pick<T>,
    ) -> Option<Vec<Effect<T>>> {
        if pressed != Some(Pressed { server, client }) {
            return None;
        }
        let mut effects = Vec::new();
        self.make_room(false, &mut effects);
        let mut session = FromX11 {
            server,
            drag,
            stage: X11Stage::Dragging,
            entered: false,
            enters: 0,
            pick: Pick::Nothing,
            proxy_shown: false,
            target: None,
            answer: (false, DragAction::None),
        };
        session.moved(pick, &mut effects);
        self.sessions.push(Session::FromX11(session));
        Some(effects)
    }

    /// A Wayland client of the seat started drag `drag`, through Smithay's grab; `data` when it
    /// has a `wl_data_source`.
    pub(crate) fn wayland_started(&mut self, drag: u64, data: bool) -> Vec<Effect<T>> {
        let mut effects = Vec::new();
        self.make_room(true, &mut effects);
        self.sessions.push(Session::FromWayland(FromWayland {
            drag,
            data,
            stage: WaylandStage::Dragging,
            x11: X11Part::Unused,
            over: None,
            status: None,
            cancelled: false,
        }));
        effects
    }

    /// The pointer moved over `pick`. Returns the focus Smithay's grab is given, for a Wayland
    /// client's drag in the hand; none when the drag in the hand is an X11 program's, or no
    /// drag is.
    pub(crate) fn moved(&mut self, pick: Pick<T>) -> (Option<Option<T>>, Vec<Effect<T>>) {
        let mut effects = Vec::new();
        let over_x11 = matches!(pick, Pick::X11 { .. });
        let in_hand = self
            .sessions
            .iter_mut()
            .rev()
            .find(|session| session.in_hand());
        let focus = match in_hand {
            Some(Session::FromX11(session)) => {
                session.moved(pick, &mut effects);
                None
            }
            Some(Session::FromWayland(session)) => Some(session.moved(pick, &mut effects)),
            None => None,
        };
        if over_x11 {
            for session in &mut self.sessions {
                if let Session::FromX11(session) = session
                    && matches!(session.stage, X11Stage::Released { .. })
                {
                    session.show_proxy(false, &mut effects);
                }
            }
        }
        (focus, effects)
    }

    /// Whether the pointer's motion matters to a drag: one is in the hand, or an X11 program's
    /// drag released shows the proxy, which a motion over an X11 window hides.
    pub(crate) fn follows_motion(&self) -> bool {
        self.in_hand()
            || self.sessions.iter().any(|session| {
                matches!(session, Session::FromX11(session)
                    if session.proxy_shown && matches!(session.stage, X11Stage::Released { .. }))
            })
    }

    /// The pointer holds no button any more, at `now` on Yuna's clock: an X11 program's drag
    /// waits for its source's drop or leave. Nothing else happens: the source decides, and finds
    /// the proxy where it was, whether it entered it already or not. A source that has not may
    /// still look for its target for the pointer's last motion, which came in the release's own
    /// round when an agent moves and releases in one call: the proxy gone, it would find the X11
    /// window under the pointer, hidden or covered by the Wayland window shown there. A Wayland
    /// client's drag follows Smithay's grab instead.
    pub(crate) fn released(&mut self, now: Duration) {
        for session in &mut self.sessions {
            if let Session::FromX11(session) = session
                && session.stage == X11Stage::Dragging
            {
                session.stage = X11Stage::Released { at: now };
            }
        }
    }

    /// The source of X11 drag `drag` of `server` entered the proxy: its types are known, and the
    /// Wayland window under the pointer is entered; entered again, with new types, a target
    /// entered is left and entered again.
    pub(crate) fn x11_entered(&mut self, server: u64, drag: u64) -> Vec<Effect<T>> {
        let mut effects = Vec::new();
        if let Some(session) = self.x11_session(server, drag)
            && session.stage == X11Stage::Dragging
        {
            if session.enters >= MAX_X11_ENTERS {
                return effects;
            }
            session.enters += 1;
            session.leave_target(&mut effects);
            session.entered = true;
            session.follow_pick(&mut effects);
        }
        effects
    }

    /// The source of X11 drag `drag` left the proxy: the Wayland target is left; after the
    /// release, the drag ends.
    pub(crate) fn x11_left(&mut self, server: u64, drag: u64) -> Vec<Effect<T>> {
        let mut effects = Vec::new();
        let Some(session) = self.x11_session(server, drag) else {
            return effects;
        };
        match session.stage {
            X11Stage::Dragging => {
                session.entered = false;
                session.leave_target(&mut effects);
            }
            X11Stage::Released { .. } => {
                session.leave_target(&mut effects);
                effects.push(Effect::Forget { server, drag });
                self.end(DragKey::FromX11 { server, drag }, &mut effects);
            }
            X11Stage::Dropped => {}
        }
        effects
    }

    /// The source of X11 drag `drag` dropped on the proxy: on the Wayland target if it is
    /// `validated`, having accepted a type and an action; otherwise refused, and the target is
    /// left.
    pub(crate) fn x11_dropped(
        &mut self,
        server: u64,
        drag: u64,
        validated: bool,
    ) -> Vec<Effect<T>> {
        let mut effects = Vec::new();
        let Some(session) = self.x11_session(server, drag) else {
            return effects;
        };
        if session.stage == X11Stage::Dropped {
            return effects;
        }
        session.show_proxy(false, &mut effects);
        let key = DragKey::FromX11 { server, drag };
        match session.target.take() {
            Some(target) if validated => {
                session.stage = X11Stage::Dropped;
                effects.push(Effect::Drop { key, target });
            }
            target => {
                session.target = target;
                session.leave_target(&mut effects);
                effects.push(Effect::Finish {
                    server,
                    drag,
                    accepted: false,
                    action: DragAction::None,
                });
                self.end(key, &mut effects);
            }
        }
        effects
    }

    /// X11 drag `drag` ended on the manager's side: its source gave up `XdndSelection` or left,
    /// another took it, or the manager ended it. A Wayland target entered is left.
    pub(crate) fn x11_ended(&mut self, server: u64, drag: u64) -> Vec<Effect<T>> {
        let mut effects = Vec::new();
        if let Some(session) = self.x11_session(server, drag) {
            session.leave_target(&mut effects);
            self.end(DragKey::FromX11 { server, drag }, &mut effects);
        }
        effects
    }

    /// The manager's proxy was mapped for X11 drag `drag`: the source is shown the pointer
    /// again, so that it finds the proxy without waiting for the next motion.
    pub(crate) fn proxy_mapped(&mut self, server: u64, drag: u64) -> Vec<Effect<T>> {
        self.proxy_now(server, drag, true)
    }

    /// The manager's proxy was unmapped while X11 drag `drag` is in the hand: the source is
    /// shown the pointer again, so that a source that looked while the proxy was still there,
    /// the pointer just come over an X11 window, looks past it, without waiting for the next
    /// motion.
    pub(crate) fn proxy_unmapped(&mut self, server: u64, drag: u64) -> Vec<Effect<T>> {
        self.proxy_now(server, drag, false)
    }

    /// The proxy is now `shown` or not for X11 drag `drag`: while the drag is in the hand and
    /// Yuna still wants the proxy so, the pointer's last motion goes again.
    fn proxy_now(&mut self, server: u64, drag: u64, shown: bool) -> Vec<Effect<T>> {
        match self.x11_session(server, drag) {
            Some(session)
                if session.stage == X11Stage::Dragging && session.proxy_shown == shown =>
            {
                vec![Effect::RepeatMotion]
            }
            _ => Vec::new(),
        }
    }

    /// The Wayland target of X11 drag `key` answers: whether it accepted a type and an action,
    /// and which action. The manager is told when it changed.
    pub(crate) fn target_answered(
        &mut self,
        key: DragKey,
        accepted: bool,
        action: DragAction,
    ) -> Vec<Effect<T>> {
        let mut effects = Vec::new();
        let DragKey::FromX11 { server, drag } = key else {
            return effects;
        };
        if let Some(session) = self.x11_session(server, drag)
            && session.stage != X11Stage::Dropped
            && session.target.is_some()
        {
            let action = if accepted { action } else { DragAction::None };
            session.tell_answer((accepted, action), &mut effects);
        }
        effects
    }

    /// The Wayland target of X11 drag `key`, dropped on, finished with `action`: the source's
    /// `XdndFinished`.
    pub(crate) fn target_finished(&mut self, key: DragKey, action: DragAction) -> Vec<Effect<T>> {
        self.after_drop(key, true, action)
    }

    /// The Wayland target of X11 drag `key`, dropped on, gave its offer up unfinished.
    pub(crate) fn target_cancelled(&mut self, key: DragKey) -> Vec<Effect<T>> {
        self.after_drop(key, false, DragAction::None)
    }

    fn after_drop(&mut self, key: DragKey, accepted: bool, action: DragAction) -> Vec<Effect<T>> {
        let mut effects = Vec::new();
        let DragKey::FromX11 { server, drag } = key else {
            return effects;
        };
        if let Some(session) = self.x11_session(server, drag)
            && session.stage == X11Stage::Dropped
        {
            let action = match (accepted, action) {
                (false, _) => DragAction::None,
                // A target always acts: Wayland's `finish` needs an action.
                (true, DragAction::None) => DragAction::Copy,
                (true, action) => action,
            };
            effects.push(Effect::Finish {
                server,
                drag,
                accepted,
                action,
            });
            self.end(key, &mut effects);
        }
        effects
    }

    /// When the first X11 drag released runs out of time waiting for its source, on Yuna's
    /// clock; none when none waits.
    pub(crate) fn next_deadline(&self) -> Option<Duration> {
        self.sessions
            .iter()
            .filter_map(|session| match session {
                Session::FromX11(FromX11 {
                    stage: X11Stage::Released { at },
                    ..
                }) => Some(at.saturating_add(X11_DROP_TIMEOUT)),
                _ => None,
            })
            .min()
    }

    /// At `now`, the X11 drags released [`X11_DROP_TIMEOUT`] ago with neither drop nor leave
    /// end: the Wayland target is left.
    pub(crate) fn timed_out(&mut self, now: Duration) -> Vec<Effect<T>> {
        let mut effects = Vec::new();
        let overdue: Vec<(u64, u64)> = self
            .sessions
            .iter()
            .filter_map(|session| match session {
                Session::FromX11(session) => match session.stage {
                    X11Stage::Released { at } if at.saturating_add(X11_DROP_TIMEOUT) <= now => {
                        Some((session.server, session.drag))
                    }
                    _ => None,
                },
                Session::FromWayland(_) => None,
            })
            .collect();
        for (server, drag) in overdue {
            if let Some(session) = self.x11_session(server, drag) {
                session.leave_target(&mut effects);
            }
            effects.push(Effect::Forget { server, drag });
            self.end(DragKey::FromX11 { server, drag }, &mut effects);
        }
        effects
    }

    /// Smithay's grab of Wayland drag `drag` was released: over an X11 window, the manager drops
    /// or leaves; elsewhere, the drag ends, and the cancel Smithay asked, `cancel_asked`, is
    /// carried out: a target that did not accept, or none. A drag no longer in the hand, ended
    /// already, its source gone, is left.
    pub(crate) fn grab_dropped(&mut self, drag: u64, cancel_asked: bool) -> Vec<Effect<T>> {
        let mut effects = Vec::new();
        let Some(session) = self.wayland_in_hand(drag) else {
            return effects;
        };
        match (session.over, session.x11) {
            (Some((server, _)), X11Part::Speaking { .. }) => {
                session.stage = WaylandStage::Dropping;
                effects.push(Effect::DropOnX11 { drag, server });
            }
            (_, x11) => {
                if let X11Part::Speaking { server } = x11 {
                    effects.push(Effect::CancelX11 { drag, server });
                }
                if cancel_asked {
                    session.cancel(&mut effects);
                }
                self.end(DragKey::FromWayland(drag), &mut effects);
            }
        }
        effects
    }

    /// Smithay's grab of Wayland drag `drag` was cancelled, not released: the drag ends, left on
    /// X11's side, and the cancel Smithay asked is carried out. A drag no longer in the hand is
    /// left.
    pub(crate) fn grab_cancelled(&mut self, drag: u64, cancel_asked: bool) -> Vec<Effect<T>> {
        let mut effects = Vec::new();
        let Some(session) = self.wayland_in_hand(drag) else {
            return effects;
        };
        if let X11Part::Speaking { server } = session.x11 {
            effects.push(Effect::CancelX11 { drag, server });
        }
        if cancel_asked {
            session.cancel(&mut effects);
        }
        self.end(DragKey::FromWayland(drag), &mut effects);
        effects
    }

    /// Whether Wayland drag `drag` is still in the hand: its decisions did not end it.
    pub(crate) fn holds_wayland(&mut self, drag: u64) -> bool {
        self.wayland_in_hand(drag).is_some()
    }

    /// Wayland drag `drag`, if it is still in the hand.
    fn wayland_in_hand(&mut self, drag: u64) -> Option<&mut FromWayland> {
        self.wayland_session(drag)
            .filter(|session| session.stage == WaylandStage::Dragging)
    }

    /// The X11 target's status for Wayland drag `drag`, from `server`'s manager: the source is
    /// told when it changed, while the drag is over an X11 window or dropping there.
    pub(crate) fn wayland_status(
        &mut self,
        server: u64,
        drag: u64,
        accepted: bool,
        action: DragAction,
    ) -> Vec<Effect<T>> {
        let mut effects = Vec::new();
        if let Some(session) = self.wayland_session(drag)
            && session.x11 == (X11Part::Speaking { server })
            && (session.stage == WaylandStage::Dropping || session.over.is_some())
        {
            let action = if accepted { action } else { DragAction::None };
            session.tell_status((accepted, action), &mut effects);
        }
        effects
    }
    /// `server`'s manager ended Wayland drag `drag` on X11's side: performed with an action, as
    /// the target's `XdndFinished` said, or not. Dropping, the source is told. Still in the hand,
    /// the drag lost `XdndSelection` to another X11 client: Smithay's grab goes on among Wayland
    /// windows, offering X11 nothing more, and its release decides.
    pub(crate) fn wayland_ended(
        &mut self,
        server: u64,
        drag: u64,
        performed: Option<DragAction>,
    ) -> Vec<Effect<T>> {
        let mut effects = Vec::new();
        let Some(session) = self.wayland_session(drag) else {
            return effects;
        };
        if session.x11 != (X11Part::Speaking { server }) {
            return effects;
        }
        match session.stage {
            WaylandStage::Dragging => session.end_x11(&mut effects),
            WaylandStage::Dropping => {
                match performed {
                    Some(action) => effects.push(Effect::Performed { drag, action }),
                    None => session.cancel(&mut effects),
                }
                self.end(DragKey::FromWayland(drag), &mut effects);
            }
        }
        effects
    }

    /// The source of Wayland drag `drag` was destroyed, or its client left: X11 is told, and
    /// the drag ends.
    pub(crate) fn wayland_source_gone(&mut self, drag: u64) -> Vec<Effect<T>> {
        let mut effects = Vec::new();
        let Some(session) = self.wayland_session(drag) else {
            return effects;
        };
        if let X11Part::Speaking { server } = session.x11 {
            effects.push(Effect::CancelX11 { drag, server });
        }
        self.end(DragKey::FromWayland(drag), &mut effects);
        effects
    }

    /// Xwayland `server` is gone: its X11 drags end, their Wayland targets left; a Wayland
    /// client's drag dropping there is cancelled, and one in the hand goes on among Wayland
    /// windows, its release deciding.
    pub(crate) fn x11_gone(&mut self, server: u64) -> Vec<Effect<T>> {
        let mut effects = Vec::new();
        let keys: Vec<DragKey> = self.sessions.iter().map(Session::key).collect();
        for key in keys {
            match key {
                DragKey::FromX11 { server: of, drag } if of == server => {
                    if let Some(session) = self.x11_session(server, drag) {
                        session.leave_target(&mut effects);
                    }
                    self.end(key, &mut effects);
                }
                DragKey::FromWayland(drag) => {
                    let Some(session) = self.wayland_session(drag) else {
                        continue;
                    };
                    if session.x11 != (X11Part::Speaking { server }) {
                        continue;
                    }
                    match session.stage {
                        WaylandStage::Dragging => session.end_x11(&mut effects),
                        WaylandStage::Dropping => {
                            session.cancel(&mut effects);
                            self.end(key, &mut effects);
                        }
                    }
                }
                DragKey::FromX11 { .. } => {}
            }
        }
        effects
    }

    /// The seat closes: every drag ends as refused.
    pub(crate) fn close(&mut self) -> Vec<Effect<T>> {
        let mut effects = Vec::new();
        for session in std::mem::take(&mut self.sessions) {
            session.refuse(&mut effects);
        }
        effects
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SERVER: u64 = 7;
    const CLIENT: u32 = 0x0040_0000;
    const PRESSED: Option<Pressed> = Some(Pressed {
        server: SERVER,
        client: CLIENT,
    });
    const SECOND: Duration = Duration::from_secs(1);

    fn x11_key(drag: u64) -> DragKey {
        DragKey::FromX11 {
            server: SERVER,
            drag,
        }
    }

    fn over_x11(window: u32) -> Pick<u32> {
        Pick::X11 {
            server: SERVER,
            window,
            point: Some((10, 20)),
        }
    }

    /// A seat with X11 drag 1 started over the X11 window it was pressed on, and its source's
    /// types known.
    fn x11_drag_entered() -> SeatDrags<u32> {
        let mut drags = SeatDrags::default();
        assert_eq!(
            drags.x11_offered(SERVER, 1, CLIENT, PRESSED, over_x11(5)),
            Some(Vec::new()),
            "over its own X11 window, the proxy stays hidden"
        );
        drags.x11_entered(SERVER, 1);
        drags
    }

    /// R2: a drag starts only from a button held on a window of the client that took
    /// `XdndSelection`, on that Xwayland.
    #[test]
    fn an_x11_drag_starts_only_from_a_button_held_on_its_clients_window() {
        let mut drags = SeatDrags::<u32>::default();
        let other_client = Some(Pressed {
            server: SERVER,
            client: 0x0080_0000,
        });
        let other_server = Some(Pressed {
            server: SERVER + 1,
            client: CLIENT,
        });
        for pressed in [None, other_client, other_server] {
            let effects = drags.x11_offered(SERVER, 1, CLIENT, pressed, Pick::Nothing);
            assert_eq!(effects, None, "{pressed:?}");
            assert!(drags.is_empty());
        }
        // Started over nothing: the proxy shows at once.
        let effects = drags.x11_offered(SERVER, 2, CLIENT, PRESSED, Pick::Nothing);
        assert_eq!(
            effects,
            Some(vec![Effect::ShowProxy {
                server: SERVER,
                drag: 2,
                shown: true
            }])
        );
        assert!(drags.in_hand());
    }

    /// R3: the proxy shows over a Wayland window and over nothing, and hides over an X11
    /// window, so that X11's own drags go on as X11 alone.
    #[test]
    fn the_proxy_shows_unless_the_drag_is_over_an_x11_window() {
        let mut drags = SeatDrags::<u32>::default();
        drags.x11_offered(SERVER, 1, CLIENT, PRESSED, over_x11(5));
        let shown = |shown| Effect::ShowProxy {
            server: SERVER,
            drag: 1,
            shown,
        };
        assert_eq!(drags.moved(Pick::Wayland(30)).1, [shown(true)]);
        assert!(drags.moved(Pick::Nothing).1.is_empty(), "shown already");
        assert_eq!(drags.moved(over_x11(6)).1, [shown(false)]);
    }

    /// An X11 program that leaves the proxy and enters it again without end offers the Wayland
    /// window under the pointer its drag [`MAX_X11_ENTERS`] times while the proxy stays shown,
    /// then nothing more; a user's drag that comes back over a Wayland window again and again,
    /// the proxy shown each time, is offered each time.
    #[test]
    fn an_x11_drag_entering_again_without_end_is_offered_a_bounded_number_of_times() {
        let entered = |effects: Vec<Effect<u32>>| {
            effects
                .iter()
                .filter(|effect| matches!(effect, Effect::Enter { .. }))
                .count()
        };
        let mut drags = SeatDrags::default();
        drags.x11_offered(SERVER, 1, CLIENT, PRESSED, over_x11(5));
        drags.moved(Pick::Wayland(30));
        let mut hostile = 0;
        for _ in 0..100 {
            drags.x11_left(SERVER, 1);
            hostile += entered(drags.x11_entered(SERVER, 1));
        }
        assert_eq!(hostile, usize::from(MAX_X11_ENTERS));

        let mut crossing = 0;
        for _ in 0..40 {
            drags.moved(over_x11(5));
            drags.x11_left(SERVER, 1);
            drags.moved(Pick::Wayland(30));
            crossing += entered(drags.x11_entered(SERVER, 1));
        }
        assert_eq!(crossing, 40, "each return over Wayland is offered");
    }

    /// The proxy after the release: kept, for a source that entered it and for one that has not
    /// yet, which may still look for its target for the pointer's last motion, made in the
    /// release's own round; until the pointer comes over an X11 window, or the drag ends, at the
    /// source's leave or drop or at [`X11_DROP_TIMEOUT`], when the manager hides it with the drag
    /// it forgets. It never takes an X11 window's pointer.
    #[test]
    fn the_proxy_hides_after_the_release_when_it_no_longer_serves() {
        let hidden = Effect::ShowProxy {
            server: SERVER,
            drag: 1,
            shown: false,
        };
        let forgotten = [
            Effect::Forget {
                server: SERVER,
                drag: 1,
            },
            Effect::Ended(x11_key(1)),
        ];
        let mut drags = x11_drag_entered();
        drags.moved(Pick::Nothing);
        drags.released(Duration::ZERO);
        assert!(drags.follows_motion(), "kept for the source");
        assert_eq!(drags.moved(over_x11(6)).1, std::slice::from_ref(&hidden));
        assert!(!drags.follows_motion());

        // Moved over a Wayland window and released in one round, before the source entered.
        let not_entered_yet = || {
            let mut drags = SeatDrags::<u32>::default();
            drags.x11_offered(SERVER, 1, CLIENT, PRESSED, over_x11(5));
            drags.moved(Pick::Wayland(30));
            drags.released(SECOND);
            assert!(drags.follows_motion(), "kept for a source yet to look");
            drags
        };
        let mut drags = not_entered_yet();
        assert!(drags.moved(Pick::Wayland(31)).1.is_empty(), "still kept");
        assert_eq!(drags.moved(over_x11(6)).1, std::slice::from_ref(&hidden));
        // The source's leave ends the drag, and the manager hides the proxy with it.
        assert_eq!(not_entered_yet().x11_left(SERVER, 1), forgotten);
        // So does the time limit.
        let mut drags = not_entered_yet();
        assert_eq!(drags.next_deadline(), Some(SECOND + X11_DROP_TIMEOUT));
        assert_eq!(drags.timed_out(SECOND + X11_DROP_TIMEOUT), forgotten);
        // Its drop, with no Wayland target entered, is refused, the proxy hidden.
        assert_eq!(
            not_entered_yet().x11_dropped(SERVER, 1, true),
            [
                hidden,
                Effect::Finish {
                    server: SERVER,
                    drag: 1,
                    accepted: false,
                    action: DragAction::None
                },
                Effect::Ended(x11_key(1))
            ]
        );
    }

    /// R4: a Wayland target is entered only once the source's `XdndEnter` reached the proxy,
    /// and left at its `XdndLeave`.
    #[test]
    fn a_wayland_target_is_entered_only_after_xdnd_enter() {
        let mut drags = SeatDrags::<u32>::default();
        drags.x11_offered(SERVER, 1, CLIENT, PRESSED, over_x11(5));
        let (focus, effects) = drags.moved(Pick::Wayland(30));
        assert_eq!(focus, None, "an X11 drag sets no grab's focus");
        assert!(
            !effects
                .iter()
                .any(|effect| matches!(effect, Effect::Enter { .. })),
            "{effects:?}"
        );
        assert_eq!(
            drags.x11_entered(SERVER, 1),
            [Effect::Enter {
                key: x11_key(1),
                target: 30
            }]
        );
        assert_eq!(
            drags.moved(Pick::Wayland(30)).1,
            [Effect::Motion {
                key: x11_key(1),
                target: 30
            }]
        );
        // Another surface: left, then entered.
        assert_eq!(
            drags.moved(Pick::Wayland(31)).1,
            [
                Effect::Leave {
                    key: x11_key(1),
                    target: 30
                },
                Effect::Enter {
                    key: x11_key(1),
                    target: 31
                }
            ]
        );
        assert_eq!(
            drags.x11_left(SERVER, 1),
            [Effect::Leave {
                key: x11_key(1),
                target: 31
            }]
        );
        assert!(drags.moved(Pick::Wayland(31)).1.is_empty(), "not entered");
        assert!(drags.in_hand(), "a leave before the release keeps the drag");
    }

    /// R5: the manager is told the target's answer when it changes, and "not accepted" once
    /// the target is left.
    #[test]
    fn the_targets_answer_reaches_the_manager_when_it_changes() {
        let mut drags = x11_drag_entered();
        drags.moved(Pick::Wayland(30));
        let answer = |accepted, action| Effect::Answer {
            server: SERVER,
            drag: 1,
            accepted,
            action,
        };
        assert_eq!(
            drags.target_answered(x11_key(1), true, DragAction::Move),
            [answer(true, DragAction::Move)]
        );
        assert!(
            drags
                .target_answered(x11_key(1), true, DragAction::Move)
                .is_empty()
        );
        assert_eq!(
            drags.moved(Pick::Nothing).1,
            [
                Effect::Leave {
                    key: x11_key(1),
                    target: 30
                },
                answer(false, DragAction::None)
            ]
        );
        assert!(
            drags
                .target_answered(x11_key(1), true, DragAction::Copy)
                .is_empty(),
            "no target, no answer"
        );
    }

    /// R7: nothing happens at the release, which gives no effect; `XdndDrop` drops on a target
    /// that accepted, and its `finish` is the source's `XdndFinished`.
    #[test]
    fn nothing_happens_at_the_release_and_the_drop_waits_for_xdnd_drop() {
        let mut drags = x11_drag_entered();
        drags.moved(Pick::Wayland(30));
        // No drop, no leave: `released` returns nothing to carry out.
        drags.released(SECOND);
        assert!(
            drags.follows_motion(),
            "the proxy kept for a motion the source has yet to read"
        );
        assert!(drags.moved(Pick::Wayland(31)).1.is_empty(), "released");
        assert_eq!(
            drags.x11_dropped(SERVER, 1, true),
            [
                Effect::ShowProxy {
                    server: SERVER,
                    drag: 1,
                    shown: false
                },
                Effect::Drop {
                    key: x11_key(1),
                    target: 30
                }
            ]
        );
        assert_eq!(drags.next_deadline(), None, "dropped: no wait of Yuna's");
        assert_eq!(
            drags.target_finished(x11_key(1), DragAction::Move),
            [
                Effect::Finish {
                    server: SERVER,
                    drag: 1,
                    accepted: true,
                    action: DragAction::Move
                },
                Effect::Ended(x11_key(1))
            ]
        );
        assert!(drags.is_empty());
    }

    /// R7: `XdndDrop` on a target that did not accept is refused, and the target left.
    #[test]
    fn a_drop_the_target_did_not_accept_is_refused() {
        let mut drags = x11_drag_entered();
        drags.moved(Pick::Wayland(30));
        drags.released(SECOND);
        assert_eq!(
            drags.x11_dropped(SERVER, 1, false),
            [
                Effect::ShowProxy {
                    server: SERVER,
                    drag: 1,
                    shown: false
                },
                Effect::Leave {
                    key: x11_key(1),
                    target: 30
                },
                Effect::Finish {
                    server: SERVER,
                    drag: 1,
                    accepted: false,
                    action: DragAction::None
                },
                Effect::Ended(x11_key(1))
            ]
        );
        // Over nothing at the drop, refused too.
        let mut drags = x11_drag_entered();
        drags.moved(Pick::Nothing);
        let effects = drags.x11_dropped(SERVER, 1, false);
        assert!(effects.contains(&Effect::Finish {
            server: SERVER,
            drag: 1,
            accepted: false,
            action: DragAction::None
        }));
    }

    /// R8: a target that gives its offer up after the drop refuses it.
    #[test]
    fn a_target_cancelled_after_the_drop_refuses_it() {
        let mut drags = x11_drag_entered();
        drags.moved(Pick::Wayland(30));
        drags.x11_dropped(SERVER, 1, true);
        assert_eq!(
            drags.target_cancelled(x11_key(1)),
            [
                Effect::Finish {
                    server: SERVER,
                    drag: 1,
                    accepted: false,
                    action: DragAction::None
                },
                Effect::Ended(x11_key(1))
            ]
        );
    }

    /// R7: released, a drag waits [`X11_DROP_TIMEOUT`] for its source's drop or leave, then
    /// ends, its target left; an `XdndLeave` after the release ends it at once.
    #[test]
    fn a_released_x11_drag_waits_five_seconds_for_its_source() {
        let mut drags = x11_drag_entered();
        drags.moved(Pick::Wayland(30));
        drags.released(SECOND);
        let due = SECOND + X11_DROP_TIMEOUT;
        assert_eq!(drags.next_deadline(), Some(due));
        assert!(drags.timed_out(due - Duration::from_millis(1)).is_empty());
        assert_eq!(
            drags.timed_out(due),
            [
                Effect::Leave {
                    key: x11_key(1),
                    target: 30
                },
                Effect::Forget {
                    server: SERVER,
                    drag: 1
                },
                Effect::Ended(x11_key(1))
            ]
        );
        assert_eq!(drags.next_deadline(), None, "at rest, no deadline");

        let mut drags = x11_drag_entered();
        drags.moved(Pick::Wayland(30));
        drags.released(SECOND);
        let effects = drags.x11_left(SERVER, 1);
        assert_eq!(effects.last(), Some(&Effect::Ended(x11_key(1))));
        assert_eq!(drags.next_deadline(), None);
    }

    /// R7: the manager ending a drag, its owner gone, leaves the target.
    #[test]
    fn an_x11_drag_ended_by_its_manager_leaves_its_target() {
        let mut drags = x11_drag_entered();
        drags.moved(Pick::Wayland(30));
        assert_eq!(
            drags.x11_ended(SERVER, 1),
            [
                Effect::Leave {
                    key: x11_key(1),
                    target: 30
                },
                Effect::Ended(x11_key(1))
            ]
        );
    }

    /// F7: the proxy mapped repeats the pointer's motion, while it is wanted.
    #[test]
    fn the_proxy_mapped_repeats_the_motion() {
        let mut drags = SeatDrags::<u32>::default();
        drags.x11_offered(SERVER, 1, CLIENT, PRESSED, Pick::Nothing);
        assert_eq!(drags.proxy_mapped(SERVER, 1), [Effect::RepeatMotion]);
        drags.moved(over_x11(5));
        assert!(drags.proxy_mapped(SERVER, 1).is_empty(), "hidden since");
        assert!(drags.proxy_mapped(SERVER, 2).is_empty(), "another drag");
    }

    /// The proxy unmapped, the drag over an X11 window, repeats the pointer's motion, so that a
    /// source that looked while the proxy was still there looks past it; not once the proxy is
    /// wanted again, nor once the drag is released.
    #[test]
    fn the_proxy_unmapped_repeats_the_motion_while_the_drag_is_in_the_hand() {
        let mut drags = SeatDrags::<u32>::default();
        drags.x11_offered(SERVER, 1, CLIENT, PRESSED, Pick::Nothing);
        assert!(drags.proxy_unmapped(SERVER, 1).is_empty(), "shown");
        drags.moved(over_x11(5));
        assert_eq!(drags.proxy_unmapped(SERVER, 1), [Effect::RepeatMotion]);
        assert!(drags.proxy_unmapped(SERVER, 2).is_empty(), "another drag");
        assert!(drags.proxy_mapped(SERVER, 1).is_empty(), "hidden");
        drags.released(SECOND);
        assert!(drags.proxy_unmapped(SERVER, 1).is_empty(), "released");
    }

    /// R10: a Wayland drag over an X11 window speaks XDND and gives Smithay's grab no focus;
    /// over a Wayland surface, the grab has it, and X11 is left.
    #[test]
    fn a_wayland_drag_over_an_x11_window_speaks_xdnd_without_a_grab_focus() {
        let mut drags = SeatDrags::<u32>::default();
        assert!(drags.wayland_started(4, true).is_empty());
        assert_eq!(drags.moved(Pick::Wayland(30)), (Some(Some(30)), Vec::new()));
        // L4: not accepted until the X11 window says so, whatever the Wayland target said.
        assert_eq!(
            drags.moved(over_x11(5)),
            (
                Some(None),
                vec![
                    Effect::Status {
                        drag: 4,
                        accepted: false,
                        action: DragAction::None
                    },
                    Effect::Over {
                        drag: 4,
                        server: SERVER,
                        over: Some((5, (10, 20)))
                    }
                ]
            )
        );
        // A point beyond X11's coordinates is not said.
        let far = Pick::X11 {
            server: SERVER,
            window: 5,
            point: None,
        };
        assert_eq!(drags.moved(far), (Some(None), Vec::new()));
        assert_eq!(
            drags.wayland_status(SERVER, 4, true, DragAction::Copy),
            [Effect::Status {
                drag: 4,
                accepted: true,
                action: DragAction::Copy
            }]
        );
        assert_eq!(
            drags.moved(Pick::Wayland(31)).1,
            [
                Effect::Over {
                    drag: 4,
                    server: SERVER,
                    over: None
                },
                Effect::Status {
                    drag: 4,
                    accepted: false,
                    action: DragAction::None
                }
            ]
        );
    }

    /// R15, R16: released over an X11 window, the manager drops; the source is told it was
    /// performed only at `XdndFinished`.
    #[test]
    fn a_wayland_drop_on_x11_is_performed_only_when_the_target_finishes() {
        let mut drags = SeatDrags::<u32>::default();
        drags.wayland_started(4, true);
        drags.moved(over_x11(5));
        drags.wayland_status(SERVER, 4, true, DragAction::Copy);
        assert_eq!(
            drags.grab_dropped(4, true),
            [Effect::DropOnX11 {
                drag: 4,
                server: SERVER
            }],
            "Smithay's cancel is held"
        );
        assert!(!drags.in_hand());
        assert_eq!(
            drags.wayland_ended(SERVER, 4, Some(DragAction::Move)),
            [
                Effect::Performed {
                    drag: 4,
                    action: DragAction::Move
                },
                Effect::Ended(DragKey::FromWayland(4))
            ]
        );
    }

    /// R18: released over an X11 window that is not XdndAware or never accepted, the source is
    /// cancelled, never told performed; a drag within one client offers X11 nothing.
    #[test]
    fn a_wayland_drop_on_x11_never_accepted_is_cancelled_not_performed() {
        let mut drags = SeatDrags::<u32>::default();
        drags.wayland_started(4, true);
        drags.moved(over_x11(5));
        drags.grab_dropped(4, true);
        let effects = drags.wayland_ended(SERVER, 4, None);
        assert_eq!(
            effects,
            [
                Effect::Cancel { drag: 4 },
                Effect::Ended(DragKey::FromWayland(4))
            ]
        );
        assert!(
            !effects
                .iter()
                .any(|effect| matches!(effect, Effect::Performed { .. }))
        );

        let mut local = SeatDrags::<u32>::default();
        local.wayland_started(5, false);
        assert_eq!(local.moved(over_x11(5)), (Some(None), Vec::new()));
        assert_eq!(
            local.grab_dropped(5, true),
            [
                Effect::Cancel { drag: 5 },
                Effect::Ended(DragKey::FromWayland(5))
            ],
            "nothing for X11"
        );
    }

    /// A Wayland drag released over a Wayland window ends; Smithay's cancel, for a target that
    /// did not accept, is carried out, and X11 is left if it was spoken to.
    #[test]
    fn a_wayland_drag_released_elsewhere_ends_with_smithays_outcome() {
        let mut drags = SeatDrags::<u32>::default();
        drags.wayland_started(4, true);
        drags.moved(over_x11(5));
        drags.moved(Pick::Wayland(30));
        assert_eq!(
            drags.grab_dropped(4, false),
            [
                Effect::CancelX11 {
                    drag: 4,
                    server: SERVER
                },
                Effect::Ended(DragKey::FromWayland(4))
            ]
        );
        let mut drags = SeatDrags::<u32>::default();
        drags.wayland_started(4, true);
        assert_eq!(
            drags.grab_cancelled(4, true),
            [
                Effect::Cancel { drag: 4 },
                Effect::Ended(DragKey::FromWayland(4))
            ]
        );
    }

    /// R17 as read: `XdndSelection` taken by another X11 client, or Xwayland gone, while the drag
    /// is in the hand ends its X11 side alone; the grab's release over the X11 window cancels the
    /// source, as Smithay asks, once. Gone while it drops, the source is cancelled at once.
    #[test]
    fn a_wayland_drags_x11_side_ending_in_the_hand_leaves_the_release_to_cancel() {
        let mut drags = SeatDrags::<u32>::default();
        drags.wayland_started(4, true);
        drags.moved(over_x11(5));
        drags.wayland_status(SERVER, 4, true, DragAction::Copy);
        assert_eq!(
            drags.wayland_ended(SERVER, 4, None),
            [Effect::Status {
                drag: 4,
                accepted: false,
                action: DragAction::None
            }]
        );
        assert!(drags.in_hand(), "the grab goes on among Wayland windows");
        assert_eq!(
            drags.moved(over_x11(5)),
            (Some(None), Vec::new()),
            "X11 is offered nothing more"
        );
        assert_eq!(
            drags.grab_dropped(4, true),
            [
                Effect::Cancel { drag: 4 },
                Effect::Ended(DragKey::FromWayland(4))
            ],
            "the release cancels"
        );
        // Xwayland gone while in the hand, then while dropping.
        let mut drags = SeatDrags::<u32>::default();
        drags.wayland_started(4, true);
        drags.moved(over_x11(5));
        assert!(
            !drags
                .x11_gone(SERVER)
                .iter()
                .any(|effect| matches!(effect, Effect::Cancel { .. }))
        );
        assert_eq!(
            drags.grab_cancelled(4, true),
            [
                Effect::Cancel { drag: 4 },
                Effect::Ended(DragKey::FromWayland(4))
            ]
        );
        let mut drags = SeatDrags::<u32>::default();
        drags.wayland_started(4, true);
        drags.moved(over_x11(5));
        drags.grab_dropped(4, true);
        assert_eq!(
            drags.x11_gone(SERVER),
            [
                Effect::Cancel { drag: 4 },
                Effect::Ended(DragKey::FromWayland(4))
            ]
        );
        assert!(
            drags.wayland_ended(SERVER, 4, None).is_empty(),
            "cancelled once"
        );
    }

    /// Decision 4: an X11 target window gone while the button is held is forgotten by the
    /// manager alone; the release over its place still goes to the manager, whose answer, no
    /// target, cancels the source, never performed.
    #[test]
    fn a_release_after_the_x11_target_went_cancels_through_the_manager() {
        let mut drags = SeatDrags::<u32>::default();
        drags.wayland_started(4, true);
        drags.moved(over_x11(5));
        drags.wayland_status(SERVER, 4, true, DragAction::Copy);
        // The window gone: the manager forgets it and says "not accepted".
        drags.wayland_status(SERVER, 4, false, DragAction::None);
        assert_eq!(
            drags.grab_dropped(4, true),
            [Effect::DropOnX11 {
                drag: 4,
                server: SERVER
            }]
        );
        assert_eq!(
            drags.wayland_ended(SERVER, 4, None),
            [
                Effect::Cancel { drag: 4 },
                Effect::Ended(DragKey::FromWayland(4))
            ]
        );
    }

    /// M1: a client starts a second drag in the same grab. Smithay ends the first grab, the
    /// oldest, at once: the first source is cancelled once, by that end alone; the second drag
    /// goes on, and its release over an X11 window that never accepted cancels it, never
    /// performed.
    #[test]
    fn a_second_drag_in_one_grab_leaves_the_first_to_smithay() {
        let mut drags = SeatDrags::<u32>::default();
        assert!(drags.wayland_started(1, true).is_empty());
        assert!(
            drags.wayland_started(2, true).is_empty(),
            "the first, in the hand, is Smithay's to end"
        );
        let first = drags.grab_cancelled(1, true);
        assert_eq!(
            first,
            [
                Effect::Cancel { drag: 1 },
                Effect::Ended(DragKey::FromWayland(1))
            ]
        );
        assert!(drags.in_hand(), "the second goes on");
        let (focus, effects) = drags.moved(over_x11(5));
        assert_eq!(focus, Some(None), "no grab focus over X11");
        assert!(effects.contains(&Effect::Over {
            drag: 2,
            server: SERVER,
            over: Some((5, (10, 20)))
        }));
        assert_eq!(
            drags.grab_dropped(2, true),
            [Effect::DropOnX11 {
                drag: 2,
                server: SERVER
            }]
        );
        let second = drags.wayland_ended(SERVER, 2, None);
        assert_eq!(
            second,
            [
                Effect::Cancel { drag: 2 },
                Effect::Ended(DragKey::FromWayland(2))
            ]
        );
        assert!(drags.is_empty());
    }

    /// R19 with M1: two Wayland drags in the hand at once, for the instant between Smithay's
    /// new grab and the end of the old, and one ending: none is refused, the ending one kept.
    #[test]
    fn a_second_drag_in_one_grab_refuses_no_ending_drag() {
        let mut drags = SeatDrags::<u32>::default();
        drags.wayland_started(1, true);
        drags.moved(over_x11(5));
        drags.grab_dropped(1, true);
        drags.wayland_started(2, true);
        assert!(drags.wayland_started(3, true).is_empty());
        assert_eq!(
            drags
                .wayland_ended(SERVER, 1, Some(DragAction::Copy))
                .first(),
            Some(&Effect::Performed {
                drag: 1,
                action: DragAction::Copy
            }),
            "the ending one still ends as its manager says"
        );
    }

    /// M1 by another way: a drag whose source is gone ends while Smithay's grab of it still
    /// runs; a second drag then started in the same grab, that grab's end names the first,
    /// ended, and leaves the second in the hand.
    #[test]
    fn the_end_of_an_ended_drags_grab_leaves_the_next_drag() {
        let mut drags = SeatDrags::<u32>::default();
        drags.wayland_started(1, true);
        assert_eq!(
            drags.wayland_source_gone(1),
            [Effect::Ended(DragKey::FromWayland(1))]
        );
        drags.wayland_started(2, true);
        assert!(!drags.holds_wayland(1), "the first ended already");
        assert!(drags.grab_cancelled(1, false).is_empty());
        assert!(drags.in_hand(), "the second goes on");
        let (focus, _) = drags.moved(over_x11(5));
        assert_eq!(
            focus,
            Some(None),
            "its X11 side still speaks: no grab focus over X11"
        );
    }

    /// R19: a new drag may start while another ends; a third start ends the oldest ending one
    /// as refused.
    #[test]
    fn a_third_drag_ends_the_oldest_ending_one_as_refused() {
        let mut drags = x11_drag_entered();
        drags.moved(Pick::Wayland(30));
        drags.x11_dropped(SERVER, 1, true);
        // A second, from Wayland, while the first waits for its target's finish.
        assert!(drags.wayland_started(4, true).is_empty());
        drags.moved(over_x11(5));
        drags.grab_dropped(4, true);
        // A third: the first, the oldest, is refused.
        let effects = drags.x11_offered(SERVER, 2, CLIENT, PRESSED, over_x11(5));
        assert_eq!(
            effects.unwrap_or_default(),
            [
                Effect::Finish {
                    server: SERVER,
                    drag: 1,
                    accepted: false,
                    action: DragAction::None
                },
                Effect::Ended(x11_key(1))
            ]
        );
        assert!(
            drags
                .target_finished(x11_key(1), DragAction::Copy)
                .is_empty()
        );
        // The second still ends as its manager says.
        assert_eq!(
            drags
                .wayland_ended(SERVER, 4, Some(DragAction::Copy))
                .first(),
            Some(&Effect::Performed {
                drag: 4,
                action: DragAction::Copy
            })
        );
    }

    /// F16: an event that names a drag no longer held, or another Xwayland's, is left.
    #[test]
    fn late_messages_are_left() {
        let mut drags = x11_drag_entered();
        drags.moved(Pick::Wayland(30));
        drags.x11_ended(SERVER, 1);
        assert!(drags.x11_entered(SERVER, 1).is_empty());
        assert!(drags.x11_dropped(SERVER, 1, true).is_empty());
        assert!(drags.x11_left(SERVER, 1).is_empty());
        assert!(
            drags
                .target_finished(x11_key(1), DragAction::Copy)
                .is_empty()
        );
        assert!(
            drags
                .wayland_status(SERVER, 9, true, DragAction::Copy)
                .is_empty()
        );
        assert!(drags.wayland_ended(SERVER, 9, None).is_empty());
        // Drag 2 of another Xwayland is not drag 2 of this one.
        let mut drags = SeatDrags::<u32>::default();
        drags.x11_offered(SERVER, 2, CLIENT, PRESSED, Pick::Nothing);
        assert!(drags.x11_entered(SERVER + 1, 2).is_empty());
        // A status from another Xwayland than the one spoken to.
        let mut drags = SeatDrags::<u32>::default();
        drags.wayland_started(4, true);
        drags.moved(over_x11(5));
        assert!(
            drags
                .wayland_status(SERVER + 1, 4, true, DragAction::Copy)
                .is_empty()
        );
    }

    /// The seat closing refuses every drag: an X11 source dropped on is told so, one in the
    /// hand forgotten, a Wayland source cancelled and left on X11's side.
    #[test]
    fn a_closing_seat_refuses_every_drag() {
        let mut drags = x11_drag_entered();
        drags.moved(Pick::Wayland(30));
        drags.x11_dropped(SERVER, 1, true);
        drags.wayland_started(4, true);
        drags.moved(over_x11(5));
        let effects = drags.close();
        assert_eq!(
            effects,
            [
                Effect::Finish {
                    server: SERVER,
                    drag: 1,
                    accepted: false,
                    action: DragAction::None
                },
                Effect::Ended(x11_key(1)),
                Effect::CancelX11 {
                    drag: 4,
                    server: SERVER
                },
                Effect::Cancel { drag: 4 },
                Effect::Ended(DragKey::FromWayland(4))
            ]
        );
        assert!(drags.is_empty());
    }
}
