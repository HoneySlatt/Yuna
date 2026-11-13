//! Text fields through `text-input-v3`, with Yuna as the input method of every agent's seat
//! ([ADR-0043](../../../docs/adr/0043-text-input-and-the-rest-of-slimeengines-examples.md)).
//!
//! A client's text field asks for an input method through `text-input-v3`. No input method
//! program runs in `yuna mcp`: Yuna is the input method of each agent's seat itself. A text field
//! is entered when its surface takes its seat's keyboard and left when the keyboard leaves; once
//! its client enables it, Yuna may send it text composed, shown in the field until replaced, and
//! text committed, which the field takes as typed. The owner's seat has no input method: a real
//! one comes with the desktop, and until then its clients' text fields are never entered.
//!
//! This file is the thin layer over Smithay's `text_input` module; `input.rs` decides what goes
//! through it.

use smithay::reexports::wayland_protocols::wp::text_input::zv3::server::zwp_text_input_v3::ZwpTextInputV3;
use smithay::reexports::wayland_server::DisplayHandle;
use smithay::wayland::text_input::{TextInputActivation, TextInputManagerState, TextInputSeat};
use yuna_layout::SeatId;

use crate::Error;
use crate::state::State;

/// The most bytes of text sent in one event, so that a message stays under libwayland's 4,096
/// bytes: longer text is committed in pieces, and longer text composed is refused.
pub const MAX_TEXT_BYTES: usize = 4000;

impl TextInputActivation for State {}

/// The `zwp_text_input_manager_v3` global, offered to every client.
pub(crate) fn text_input_global(display: &DisplayHandle) -> TextInputManagerState {
    TextInputManagerState::new::<State>(display)
}

/// Makes Yuna the input method of an agent's seat, before any client has its keyboard.
pub(crate) fn act_as_input_method(seat: &smithay::input::Seat<State>) {
    seat.text_input().set_compositor_input_method(true);
}

impl State {
    /// Whether a text field is enabled where `seat`'s keyboard is.
    pub(crate) fn text_field_enabled(&self, seat: SeatId) -> bool {
        let Some(data) = self.seat(seat) else {
            return false;
        };
        let mut enabled = false;
        data.seat
            .text_input()
            .with_active_text_input(|_, _| enabled = true);
        enabled
    }

    /// Gives the text field enabled where `seat`'s keyboard is `text` as committed, replacing
    /// any text being composed, in pieces of [`MAX_TEXT_BYTES`] at most. Refused when no text
    /// field is enabled there.
    pub(crate) fn commit_text(&mut self, seat: SeatId, text: &str) -> Result<(), Error> {
        let mut rest = text;
        while !rest.is_empty() {
            let mut end = rest.len().min(MAX_TEXT_BYTES);
            while !rest.is_char_boundary(end) {
                end -= 1;
            }
            let (piece, after) = rest.split_at(end);
            self.send_to_text_field(seat, |field| field.commit_string(Some(piece.to_owned())))?;
            rest = after;
        }
        Ok(())
    }

    /// Shows `text` as being composed in the text field enabled where `seat`'s keyboard is, its
    /// cursor at its end; empty text clears it. Refused when no text field is enabled there,
    /// and past [`MAX_TEXT_BYTES`].
    pub(crate) fn compose_text(&mut self, seat: SeatId, text: &str) -> Result<(), Error> {
        if text.len() > MAX_TEXT_BYTES {
            return Err(Error::Refused(format!(
                "text composed holds {MAX_TEXT_BYTES} bytes at most"
            )));
        }
        let end = i32::try_from(text.len()).unwrap_or(i32::MAX);
        self.send_to_text_field(seat, |field| {
            if !text.is_empty() {
                field.preedit_string(Some(text.to_owned()), end, end);
            }
        })
    }

    /// Sends `events` to the enabled text field of `seat`, then `done`, which applies them: a
    /// `done` without a preedit string clears the one shown.
    fn send_to_text_field(
        &mut self,
        seat: SeatId,
        events: impl Fn(&ZwpTextInputV3),
    ) -> Result<(), Error> {
        if !self.text_field_enabled(seat) {
            return Err(Error::Refused(
                "no text field is enabled where the keyboard is".to_owned(),
            ));
        }
        let Some(data) = self.seat(seat) else {
            return Err(Error::Refused(format!("no seat {}", seat.0)));
        };
        let text_input = data.seat.text_input();
        text_input.with_active_text_input(|field, _| events(field));
        text_input.done(false);
        Ok(())
    }
}
