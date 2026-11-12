//! Popups (`xdg_popup`): menus and tooltips, placed by their positioner within the output their
//! window is shown on, drawn just above their window, called back with it, and dismissed with it.
//!
//! Smithay's [`PopupManager`] keeps each window's popups in a tree on its toplevel's surface, each
//! with its place relative to the toplevel's window geometry. Popups are not windows of the
//! layout: they get no `WindowId` and never change it.
//!
//! A popup is placed when it is created and when it asks to be placed again (`reposition`): the
//! positioner's rules, then its constraint adjustments against the output its window is shown
//! on. Its initial commit gets its configure; its first buffer shows it. Its commits arm its
//! window's output, and its frame callbacks and presentation feedback go with its window's
//! (`frames.rs`), at the hidden windows' rate when the window is hidden.
//!
//! A popup that asks an explicit grab gets it when the grab's serial is the last keyboard `enter`
//! the owner's seat sent to the popup's window, or the serial of the grab its parent holds; there
//! is no pointer input before M1.5. The keyboard then goes to the topmost grabbing popup, until
//! the grab ends: when its popups are gone, or when the focus leaves their window. A grab asked
//! otherwise is refused by dismissing the popup, as xdg-shell allows.
//!
//! A popup is dismissed (`popup_done`) when its window unmaps or goes, and at once when it is
//! created for a window that is not mapped, a parent popup already dismissed, a chain of parents
//! that is broken or deeper than [`MAX_POPUP_DEPTH`], or a positioner with nonsense values.

use smithay::desktop::{
    PopupGrab, PopupGrabError, PopupKeyboardGrab, PopupKind, PopupManager, PopupUngrabStrategy,
};
use smithay::reexports::wayland_server::Resource;
use smithay::reexports::wayland_server::protocol::wl_seat::WlSeat;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{Logical, Point, Rectangle, Serial};
use smithay::wayland::compositor::{get_role, with_states};
use smithay::wayland::shell::xdg::{
    PopupSurface, PositionerState, XDG_POPUP_ROLE, XdgPopupSurfaceData,
};
use yuna_layout::{OutputId, WindowId};

use crate::state::State;

/// The deepest chain of popups accepted: menus nest a few levels. A deeper chain is a client's
/// nonsense, or a cycle of parents, which Smithay neither refuses nor survives: its walks up a
/// popup's parents and down a window's tree have no bound.
pub const MAX_POPUP_DEPTH: usize = 32;

/// The largest coordinate or size, in logical units, a positioner may hold. Placing a popup adds a
/// few of them; within this bound the sums stay far from `i32`'s limits.
const MAX_POSITIONER_VALUE: u32 = 1 << 20;

/// A granted popup grab, and the toplevel surface whose popups it holds.
pub(crate) struct ActivePopupGrab {
    pub toplevel: WlSurface,
    pub grab: PopupGrab<State>,
}

impl State {
    /// A new popup: placed and tracked, or dismissed when it cannot be placed.
    pub(crate) fn popup_created(&mut self, popup: PopupSurface, positioner: PositionerState) {
        let Some(geometry) = self.place_popup(&popup, &positioner) else {
            tracing::debug!(
                "a popup without a mapped window, or with nonsense values, is dismissed"
            );
            popup.send_popup_done();
            return;
        };
        popup.with_pending_state(|state| {
            state.geometry = geometry;
            state.positioner = positioner;
        });
        if let Err(error) = self.popups.track_popup(PopupKind::Xdg(popup.clone())) {
            tracing::debug!(%error, "a popup could not be tracked; it is dismissed");
            popup.send_popup_done();
        }
    }

    /// A commit of `surface`, part of the popup whose surface is `popup_surface`. Its initial
    /// commit gets its configure; every commit arms what its window's commits arm.
    pub(crate) fn popup_committed(&mut self, popup_surface: &WlSurface, surface: &WlSurface) {
        self.popups.commit(popup_surface);
        // A dismissed popup is no longer tracked: its client was told to destroy it.
        let Some(PopupKind::Xdg(popup)) = self.popups.find_popup(popup_surface) else {
            return;
        };
        if !popup.is_initial_configure_sent() {
            match popup.send_configure() {
                Ok(_) => self.counters.configures += 1,
                Err(error) => tracing::debug!(%error, "a popup's configure was refused"),
            }
        }
        if let Some(toplevel) = popup_toplevel(popup_surface) {
            self.schedule_for_commit(&toplevel, surface);
        }
    }

    /// A popup asks to be placed again with a new positioner: it is, and told so with `token`.
    pub(crate) fn popup_reposition_requested(
        &mut self,
        popup: PopupSurface,
        positioner: PositionerState,
        token: u32,
    ) {
        if self.popups.find_popup(popup.wl_surface()).is_none() {
            return;
        }
        match self.place_popup(&popup, &positioner) {
            Some(geometry) => {
                popup.with_pending_state(|state| {
                    state.geometry = geometry;
                    state.positioner = positioner;
                });
                popup.send_repositioned(token);
                self.counters.configures += 1;
            }
            None => {
                if let Some(toplevel) = popup_toplevel(popup.wl_surface()) {
                    self.dismiss_popup(&toplevel, &popup);
                }
            }
        }
    }

    /// A popup's `xdg_popup` or its surface is gone: its window's output draws again without it,
    /// and a grab it held may end.
    pub(crate) fn popup_gone(&mut self, popup_surface: &WlSurface) {
        if let Some(toplevel) = popup_toplevel(popup_surface) {
            self.refresh_output_of(&toplevel);
        }
        if self.popup_grab.is_some() {
            self.focus_keyboard();
        }
    }

    /// An explicit grab asked by a popup: granted, or refused by dismissing the popup. See the
    /// module's documentation.
    pub(crate) fn popup_grab_requested(
        &mut self,
        popup: PopupSurface,
        seat: WlSeat,
        serial: Serial,
    ) {
        let Some(toplevel) = popup_toplevel(popup.wl_surface()) else {
            popup.send_popup_done();
            return;
        };
        let seat = self.owner_seat_of(&seat);
        let Some(seat) = seat.filter(|_| self.grab_serial_is_valid(&toplevel, serial)) else {
            tracing::debug!("a popup grab with a serial Yuna's seat did not send is refused");
            self.dismiss_popup(&toplevel, &popup);
            return;
        };
        // Smithay refuses, with a protocol error, a grab whose parent is not the topmost grabbing
        // popup; xdg-shell allows a new menu on the window, so the older grab ends first.
        let parent = popup.get_parent_surface();
        if self
            .popup_grab
            .as_ref()
            .is_some_and(|active| active.grab.current_grab() != parent)
        {
            self.end_popup_grab();
        }
        let kind = PopupKind::Xdg(popup.clone());
        match self
            .popups
            .grab_popup(toplevel.clone(), kind, &seat, serial)
        {
            Ok(grab) => {
                let keyboard = self.keyboard.clone();
                keyboard.set_grab(self, PopupKeyboardGrab::new(&grab), serial);
                self.popup_grab = Some(ActivePopupGrab { toplevel, grab });
                self.focus_keyboard();
            }
            // Smithay dismissed the popup already.
            Err(PopupGrabError::ParentDismissed) => {}
            // Smithay sent the client a protocol error.
            Err(PopupGrabError::NotTheTopmostPopup) => {}
            Err(error) => {
                tracing::debug!(%error, "a popup grab is refused");
                self.dismiss_popup(&toplevel, &popup);
            }
        }
    }

    /// What the keyboard focuses, given the toplevel surface the layout focuses: the topmost
    /// popup of a grab on that window, or the window. A grab whose popups are gone, or whose
    /// window no longer has the focus, ends here: its popups are dismissed and the keyboard is let
    /// go.
    pub(crate) fn keyboard_focus(&mut self, window: Option<WlSurface>) -> Option<WlSurface> {
        let Some(active) = &self.popup_grab else {
            return window;
        };
        let topmost = active.grab.current_grab();
        let holds = !active.grab.has_ended()
            && window.as_ref() == Some(&active.toplevel)
            && topmost.as_ref() != Some(&active.toplevel);
        if holds {
            return topmost;
        }
        self.end_popup_grab();
        window
    }

    /// Dismisses every popup of the toplevel whose surface is `toplevel`, which unmaps or goes,
    /// and ends a grab they held.
    pub(crate) fn dismiss_popups_of(&mut self, toplevel: &WlSurface) {
        let popups: Vec<PopupKind> = PopupManager::popups_for_surface(toplevel)
            .map(|(popup, _)| popup)
            .collect();
        // Topmost first, so that each popup is told once: dismissing one tells the popups over it.
        for popup in popups {
            if PopupManager::dismiss_popup(toplevel, &popup).is_err()
                && let PopupKind::Xdg(popup) = popup
            {
                // The toplevel's surface is gone, and its tree with it: each popup is told here.
                popup.send_popup_done();
            }
        }
        if self
            .popup_grab
            .as_ref()
            .is_some_and(|active| active.toplevel == *toplevel)
        {
            self.end_popup_grab();
        }
    }

    /// Where a popup goes, relative to its parent's window geometry: by its positioner, kept
    /// within the output its window is shown on as its constraint adjustments allow. None when it
    /// cannot be placed: nonsense values, no mapped window, or a parent popup no longer tracked.
    fn place_popup(
        &self,
        popup: &PopupSurface,
        positioner: &PositionerState,
    ) -> Option<Rectangle<i32, Logical>> {
        if !positioner_is_sane(positioner) {
            return None;
        }
        let toplevel_surface = popup_toplevel(popup.wl_surface())?;
        let toplevel = self.toplevels.get(&toplevel_surface.id())?;
        let window = toplevel.window?;
        let parent = popup.get_parent_surface()?;
        // Where the parent's window geometry is, relative to the toplevel's.
        let parent_offset = if parent == toplevel_surface {
            Point::default()
        } else {
            PopupManager::popups_for_surface(&toplevel_surface)
                .find(|(tracked, _)| *tracked.wl_surface() == parent)
                .map(|(_, at)| at)?
        };
        // A window shown on no output has nothing to keep its popups within.
        let Some(output) = self.window_output_rect(window, toplevel.shown_on) else {
            return Some(positioner.get_geometry());
        };
        let target = Rectangle::new(output.loc - parent_offset, output.size);
        Some(positioner.get_unconstrained_geometry(target))
    }

    /// The output `window` is shown on, as a rectangle relative to the window geometry's
    /// top-left corner, in whole logical units, from the last arrangement.
    fn window_output_rect(
        &self,
        window: WindowId,
        shown_on: Option<OutputId>,
    ) -> Option<Rectangle<i32, Logical>> {
        let output = &self.outputs[self.output_index(shown_on?)?];
        let placed = output
            .placed
            .iter()
            .find(|placed| placed.window == window)?;
        // `as` saturates: an output far away still gives a rectangle, only a wrong one.
        let x = (output.rect.x - placed.rect.x).round() as i32;
        let y = (output.rect.y - placed.rect.y).round() as i32;
        let size = (output.rect.w.round() as i32, output.rect.h.round() as i32);
        Some(Rectangle::new((x, y).into(), size.into()))
    }

    /// Whether `serial` may start a popup grab on the toplevel whose surface is `toplevel`: the
    /// keyboard is on that window or one of its popups, and the serial is the last `enter` the
    /// keyboard sent, or the serial of the grab a parent popup holds.
    fn grab_serial_is_valid(&self, toplevel: &WlSurface, serial: Serial) -> bool {
        let focus_in_window = self.keyboard.current_focus().is_some_and(|focus| {
            focus == *toplevel || popup_toplevel(&focus).as_ref() == Some(toplevel)
        });
        focus_in_window
            && (self.keyboard.last_enter() == Some(serial) || self.keyboard.has_grab(serial))
    }

    /// Dismisses `popup` and the popups over it, which leave their window's tree.
    fn dismiss_popup(&mut self, toplevel: &WlSurface, popup: &PopupSurface) {
        let kind = PopupKind::Xdg(popup.clone());
        let tracked =
            PopupManager::popups_for_surface(toplevel).any(|(tracked, _)| tracked == kind);
        if !tracked || PopupManager::dismiss_popup(toplevel, &kind).is_err() {
            popup.send_popup_done();
        }
        self.refresh_output_of(toplevel);
    }

    /// Ends the popup grab: its popups still open are dismissed, and the keyboard is let go. The
    /// focus moves at the next call of `focus_keyboard`.
    fn end_popup_grab(&mut self) {
        if let Some(mut active) = self.popup_grab.take() {
            active.grab.ungrab(PopupUngrabStrategy::All);
            let keyboard = self.keyboard.clone();
            keyboard.unset_grab(self);
        }
    }

    /// Arms the next boundary of the output the toplevel whose surface is `toplevel` is shown on,
    /// so that a popup that left it is no longer drawn.
    fn refresh_output_of(&mut self, toplevel: &WlSurface) {
        if let Some(output) = self
            .toplevels
            .get(&toplevel.id())
            .and_then(|toplevel| toplevel.shown_on)
        {
            self.schedule_refresh(output);
        }
    }
}

/// The surfaces whose trees make a window: its toplevel's surface, then each of its popups',
/// topmost first.
pub(crate) fn window_surface_trees(toplevel: &WlSurface) -> Vec<WlSurface> {
    std::iter::once(toplevel.clone())
        .chain(
            PopupManager::popups_for_surface(toplevel).map(|(popup, _)| popup.wl_surface().clone()),
        )
        .collect()
}

/// The toplevel surface a popup belongs to, through its parent popups. None for a surface that
/// is not a popup, or whose chain of parents is broken (a parent destroyed, or never set),
/// cyclic, or deeper than [`MAX_POPUP_DEPTH`].
pub(crate) fn popup_toplevel(popup: &WlSurface) -> Option<WlSurface> {
    let mut surface = popup.clone();
    for _ in 0..=MAX_POPUP_DEPTH {
        if get_role(&surface) != Some(XDG_POPUP_ROLE) {
            return (surface != *popup).then_some(surface);
        }
        surface = popup_parent(&surface)?;
    }
    None
}

/// The parent a popup surface was created with; none once its `xdg_popup` is destroyed.
fn popup_parent(popup: &WlSurface) -> Option<WlSurface> {
    with_states(popup, |states| {
        let role = states.data_map.get::<XdgPopupSurfaceData>()?.lock().ok()?;
        role.parent.clone()
    })
}

/// Whether a positioner's values can be placed: a size set, and every value within
/// [`MAX_POSITIONER_VALUE`].
fn positioner_is_sane(positioner: &PositionerState) -> bool {
    let size = positioner.rect_size;
    let anchor = positioner.anchor_rect;
    let values = [
        size.w,
        size.h,
        anchor.loc.x,
        anchor.loc.y,
        anchor.size.w,
        anchor.size.h,
        positioner.offset.x,
        positioner.offset.y,
    ];
    size.w > 0
        && size.h > 0
        && values
            .iter()
            .all(|value| value.unsigned_abs() <= MAX_POSITIONER_VALUE)
}
