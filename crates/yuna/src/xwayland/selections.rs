//! The clipboard and the primary selection between a seat's X11 programs and its Wayland
//! programs, on Yuna's side: what each seat's X11 window manager posts of X11's selections
//! becomes the seat's Wayland selections, which Yuna holds itself (`selection.rs`), and what
//! Wayland's side sets, a client or an agent, becomes the manager's commands. `bridge.rs` holds
//! the X11 side, and tells the whole flow and its bounds.
//!
//! Yuna's loop never reads nor writes the data of X11, and never waits: a Wayland reader's pipe
//! goes to the manager with its command, and a pipe to a Wayland source is made, given to the
//! source, a client or Yuna's own writer of an agent's text, and its read end sent to the
//! manager. It counts the transfers to Wayland it passed on, so that a client's reads past
//! [`MAX_X11_TRANSFERS`] are refused here, never queued.

use std::os::fd::OwnedFd;

use yuna_types::SeatId;

use super::bridge::{MAX_X11_TRANSFERS, Selection};
use super::manager::{Command, ManagerEvent};
use crate::selection::{HeldSelection, X11Offer};
use crate::state::State;

impl super::X11Windows {
    /// Whether an Xwayland serves `seat`.
    pub(crate) fn serves(&self, seat: SeatId) -> bool {
        self.servers.values().any(|record| record.seat == seat)
    }
}

impl State {
    /// What Xwayland `server`'s manager posted of the seat's selections.
    pub(super) fn x11_selection_event(&mut self, server: u64, event: ManagerEvent) {
        let Some(seat) = self.x11.servers.get(&server).map(|record| record.seat) else {
            return;
        };
        match event {
            ManagerEvent::SelectionTaken {
                selection,
                owner,
                mime_types,
                wayland_serial,
            } => {
                // A Wayland client's selection newer than the manager knew of is the latest:
                // the manager takes X11's selection for it from that owner.
                let newer = self
                    .selections
                    .set_on_wayland(seat, selection)
                    .is_some_and(|set| set.mime_types.is_some() && set.serial != wayland_serial);
                if newer {
                    return;
                }
                if mime_types.is_empty() {
                    self.clear_selection_from_x11(seat, selection);
                } else {
                    let offer = X11Offer {
                        server,
                        selection,
                        owner,
                    };
                    self.hold_x11_selection(seat, selection, mime_types, offer);
                }
            }
            ManagerEvent::SelectionDropped(selection) => {
                self.drop_x11_selection(seat, selection, server);
            }
            ManagerEvent::ReadWayland {
                selection,
                mime_type,
                transfer,
            } => {
                let source = self.read_wayland_selection(seat, selection, mime_type);
                self.x11
                    .send(server, Command::WaylandData { transfer, source });
            }
            ManagerEvent::TransferToWaylandEnded => {
                if let Some(record) = self.x11.servers.get_mut(&server) {
                    record.transfers_to_wayland = record.transfers_to_wayland.saturating_sub(1);
                }
            }
            _ => {}
        }
    }

    /// Empties `seat`'s `selection` if Yuna holds it for an X11 owner of Xwayland `server`.
    fn drop_x11_selection(&mut self, seat: SeatId, selection: Selection, server: u64) {
        if let Some(HeldSelection::X11 { offer, .. }) = self.held_selection(seat, selection)
            && offer.server == server
        {
            self.clear_selection_from_x11(seat, selection);
        }
    }

    /// Xwayland `server`'s manager took charge: it is told the seat's Wayland selections that
    /// clients or agents set, still there, so that an X11 program started after a copy pastes
    /// it.
    pub(super) fn x11_selections_ready(&mut self, server: u64) {
        let Some(seat) = self.x11.servers.get(&server).map(|record| record.seat) else {
            return;
        };
        for selection in Selection::BOTH {
            let Some(set) = self.selections.set_on_wayland(seat, selection).cloned() else {
                continue;
            };
            let Some(mime_types) = set.mime_types else {
                continue;
            };
            self.x11.send(
                server,
                Command::OwnSelection {
                    selection,
                    serial: set.serial,
                    mime_types: Some(mime_types),
                },
            );
        }
    }

    /// Xwayland `server` is ending: the selections Yuna held for its X11 clients empty.
    pub(super) fn x11_selections_gone(&mut self, server: u64) {
        let Some(seat) = self.x11.servers.get(&server).map(|record| record.seat) else {
            return;
        };
        for selection in Selection::BOTH {
            self.drop_x11_selection(seat, selection, server);
        }
    }

    /// A Wayland client, or an agent through MCP's `clipboard get`, reads a selection Yuna holds
    /// for an X11 owner, as `mime_type`, into `pipe`: the pipe goes to the owner's manager,
    /// unless its Xwayland is gone or
    /// [`MAX_X11_TRANSFERS`] run already; then it closes, and the reader sees it end.
    pub(crate) fn x11_send_selection(&mut self, offer: X11Offer, mime_type: String, pipe: OwnedFd) {
        let Some(record) = self.x11.servers.get_mut(&offer.server) else {
            return;
        };
        if record.transfers_to_wayland >= MAX_X11_TRANSFERS {
            tracing::debug!(
                seat = record.seat.0,
                "a Wayland client's read of an X11 selection is refused: too many transfers run"
            );
            return;
        }
        record.transfers_to_wayland += 1;
        self.x11.send(
            offer.server,
            Command::SendToWayland {
                selection: offer.selection,
                owner: offer.owner,
                mime_type,
                pipe,
            },
        );
    }

    /// `seat`'s `selection`, as Wayland's side sets it, changed: each Xwayland of the seat is
    /// told at the round's end, once however often it changed.
    pub(crate) fn x11_wayland_selection_changed(&mut self, seat: SeatId, selection: Selection) {
        if self.x11.serves(seat) && !self.x11.wayland_changed.contains(&(seat, selection)) {
            self.x11.wayland_changed.push((seat, selection));
        }
    }

    /// Puts, for each Xwayland, the latest of its seat's Wayland selections that changed this
    /// round in its commands: what a client or an agent set, or none.
    pub(super) fn x11_selection_commands(&mut self) {
        for (seat, selection) in std::mem::take(&mut self.x11.wayland_changed) {
            let Some(set) = self.selections.set_on_wayland(seat, selection).cloned() else {
                continue;
            };
            let servers: Vec<u64> = self
                .x11
                .servers
                .iter()
                .filter(|(_, record)| record.seat == seat)
                .map(|(&server, _)| server)
                .collect();
            for server in servers {
                self.x11.send(
                    server,
                    Command::OwnSelection {
                        selection,
                        serial: set.serial,
                        mime_types: set.mime_types.clone(),
                    },
                );
            }
        }
    }
}
