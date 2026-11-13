//! `xdg-activation-v1`: a client asks a token, then asks with it that a window be activated, as a
//! launcher does for the program it starts
//! ([ADR-0026](../../../docs/adr/0026-real-clients-in-the-layout.md)).
//!
//! A token is valid when it was asked with its client's own seat, for a surface of that client, and
//! a serial the seat sent there: the last keyboard `enter` or key press while the seat's keyboard
//! is on that surface, or a button press on the surface's window. A valid token lasts
//! [`TOKEN_LIFETIME`] on Yuna's clock and is used once, for a window of the same seat. A token
//! asked otherwise is still given, since the protocol requires one, but is not valid.
//!
//! An activation with a valid token asks the layout ([`Layout::request_activation`]): the window
//! takes the focus at the next refresh, unless a fullscreen window holds the screen.
//! Refused, or with a token that is not valid, the window is marked urgent and nothing else
//! changes, so that no program takes a game's screen. Urgent is the compositor's state, kept on
//! the toplevel: it ends when the window gets its seat's focus, or closes. A request for a
//! toplevel not mapped yet waits for its mapping, and is judged then: a token that expired
//! meanwhile only marks the window urgent.
//!
//! [`Layout::request_activation`]: yuna_layout::Layout::request_activation

use std::time::Duration;

use smithay::reexports::wayland_server::Resource;
use smithay::reexports::wayland_server::backend::ClientId;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::wayland::xdg_activation::{
    XdgActivationHandler, XdgActivationState, XdgActivationToken, XdgActivationTokenData,
};
use yuna_layout::{SeatId, WindowId};

use crate::input::window_of_surface;
use crate::seats::{seat_of_surface, seat_of_wl_seat};
use crate::state::{OWNER_SEAT, State};

/// How long a valid token stays valid after it was asked, on Yuna's clock.
pub const TOKEN_LIFETIME: Duration = Duration::from_secs(10);

/// The most tokens kept for one client. A token is kept until it is used or its client asked
/// this many newer ones, so that a client asking tokens in a loop can neither fill memory nor
/// push out another client's tokens; and without a timer, so that nothing runs at rest. Until
/// then an expired token is still known, and marks its window urgent.
const MAX_KEPT_TOKENS_PER_CLIENT: usize = 64;

/// What Yuna noted of a token it gave, kept in the token's data, and on a toplevel whose
/// activation waits for its mapping.
#[derive(Debug, Clone, Copy)]
pub(crate) struct TokenOrigin {
    /// When it was asked, on Yuna's clock.
    asked_at: Duration,
    /// Whether it was asked as a valid token must be.
    asked_validly: bool,
    /// The seat of the client that asked it: it activates only that seat's windows.
    seat: SeatId,
}

impl TokenOrigin {
    /// A token Yuna noted nothing of: never valid.
    const UNKNOWN: TokenOrigin = TokenOrigin {
        asked_at: Duration::ZERO,
        asked_validly: false,
        seat: OWNER_SEAT,
    };

    /// Whether an activation with this token at `now` of a window of `seat` may take the focus:
    /// asked validly, by a client of that seat, less than [`TOKEN_LIFETIME`] before `now`.
    fn is_valid_at(&self, now: Duration, seat: SeatId) -> bool {
        self.asked_validly
            && self.seat == seat
            && now < self.asked_at.saturating_add(TOKEN_LIFETIME)
    }
}

impl XdgActivationHandler for State {
    fn activation_state(&mut self) -> &mut XdgActivationState {
        &mut self.xdg_activation_state
    }

    /// Notes when and how the token was asked. Every token is kept: one asked wrongly must still
    /// reach [`XdgActivationHandler::request_activation`], which marks its window urgent; Smithay
    /// passes on only the tokens it keeps.
    fn token_created(&mut self, _token: XdgActivationToken, data: XdgActivationTokenData) -> bool {
        let seat = data
            .client_id
            .as_ref()
            .and_then(|client| self.seat_of_client_id(client))
            .unwrap_or(OWNER_SEAT);
        let origin = TokenOrigin {
            asked_at: self.clock.now(),
            asked_validly: self.token_asked_validly(seat, &data),
            seat,
        };
        // The data is shared with the copy Smithay keeps.
        data.user_data.insert_if_missing_threadsafe(|| origin);
        self.forget_oldest_tokens_of(data.client_id.as_ref());
        true
    }

    fn request_activation(
        &mut self,
        token: XdgActivationToken,
        token_data: XdgActivationTokenData,
        surface: WlSurface,
    ) {
        // A token is used once.
        self.xdg_activation_state.remove_token(&token);
        let origin = token_data
            .user_data
            .get::<TokenOrigin>()
            .copied()
            .unwrap_or(TokenOrigin::UNKNOWN);
        let Some(toplevel) = self.toplevels.get_mut(&surface.id()) else {
            tracing::debug!("an activation of a surface that is not a toplevel is ignored");
            return;
        };
        match toplevel.window {
            Some(window) => self.activate(window, origin),
            // Judged when it maps (`shell.rs`), by when its token was asked.
            None => toplevel.waiting_activation = Some(origin),
        }
    }
}

impl State {
    /// An activation of a mapped window with `token`: the layout decides if the token is valid
    /// now; refused, or without a valid token, the window is urgent.
    pub(crate) fn activate(&mut self, window: WindowId, token: TokenOrigin) {
        let seat = self.layout.window(window).map(|info| info.seat);
        if seat.is_some_and(|seat| token.is_valid_at(self.clock.now(), seat)) {
            match self.layout.request_activation(window) {
                Ok(true) => {
                    self.mark_layout_changed();
                    return;
                }
                Ok(false) => {
                    tracing::debug!(window = window.0, "a fullscreen window holds the screen");
                }
                Err(error) => tracing::debug!(%error, window = window.0, "activation refused"),
            }
        }
        self.mark_urgent(window);
    }

    /// Whether `window` is urgent: an activation of it was refused, or came without a valid
    /// token, and it has not had the seat's focus since.
    pub(crate) fn is_urgent(&self, window: WindowId) -> bool {
        self.windows
            .get(&window)
            .and_then(|surface| self.toplevels.get(surface))
            .is_some_and(|toplevel| toplevel.urgent)
    }

    /// Ends the urgency of the window each seat focuses: it has its user's attention.
    pub(crate) fn clear_urgency_of_focused(&mut self) {
        for seat in self.seat_ids() {
            let focused = self
                .layout
                .focused(seat)
                .and_then(|window| self.windows.get(&window));
            if let Some(toplevel) = focused.and_then(|surface| self.toplevels.get_mut(surface)) {
                toplevel.urgent = false;
            }
        }
    }

    fn mark_urgent(&mut self, window: WindowId) {
        // A window that has its seat's focus already needs no one's attention.
        let seat = self.layout.window(window).map(|info| info.seat);
        if seat.is_some_and(|seat| self.layout.focused(seat) == Some(window)) {
            return;
        }
        let toplevel = self
            .windows
            .get(&window)
            .and_then(|surface| self.toplevels.get_mut(surface));
        if let Some(toplevel) = toplevel {
            toplevel.urgent = true;
            tracing::debug!(window = window.0, "window marked urgent");
        }
    }

    /// Whether a token asked by a client of `client_seat` was asked as a valid one must be: with
    /// that seat, for a surface of that client, and a serial the seat sent there (see the
    /// module's documentation).
    fn token_asked_validly(&self, client_seat: SeatId, data: &XdgActivationTokenData) -> bool {
        let (Some((serial, seat)), Some(surface)) = (&data.serial, &data.surface) else {
            return false;
        };
        if seat_of_wl_seat(seat) != Some(client_seat) || seat_of_surface(surface) != client_seat {
            return false;
        }
        let Some(seat) = self.seat(client_seat) else {
            return false;
        };
        let keyboard_on_surface = seat.keyboard.current_focus().as_ref() == Some(surface);
        let key_pressed = seat
            .last_key_press
            .as_ref()
            .is_some_and(|(pressed, _)| pressed == serial);
        let pressed_on_window = seat
            .last_button_press
            .as_ref()
            .is_some_and(|(pressed, window)| {
                pressed == serial && window_of_surface(surface).as_ref() == Some(window)
            });
        pressed_on_window
            || (keyboard_on_surface && (seat.keyboard.last_enter() == Some(*serial) || key_pressed))
    }

    /// Forgets the oldest tokens of `client`, the client asking a new one, until it has room for
    /// one more within [`MAX_KEPT_TOKENS_PER_CLIENT`]. Other clients' tokens are left alone.
    fn forget_oldest_tokens_of(&mut self, client: Option<&ClientId>) {
        let asked_at = |data: &XdgActivationTokenData| {
            data.user_data
                .get::<TokenOrigin>()
                .map_or(Duration::ZERO, |origin| origin.asked_at)
        };
        let of_client = |data: &XdgActivationTokenData| data.client_id.as_ref() == client;
        let kept =
            |state: &XdgActivationState| state.tokens().filter(|(_, data)| of_client(data)).count();
        while kept(&self.xdg_activation_state) >= MAX_KEPT_TOKENS_PER_CLIENT {
            let oldest = self
                .xdg_activation_state
                .tokens()
                .filter(|(_, data)| of_client(data))
                .min_by_key(|(_, data)| asked_at(data))
                .map(|(token, _)| token.clone());
            match oldest {
                Some(token) => {
                    self.xdg_activation_state.remove_token(&token);
                }
                None => break,
            }
        }
    }
}
