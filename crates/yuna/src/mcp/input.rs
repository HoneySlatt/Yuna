//! The `pointer` and `keyboard` tools
//! ([ADR-0095](../../../../docs/adr/0095-an-agents-seat-through-mcp-its-tools-input-keyboard-layouts-and-closing.md)):
//! a seat's pointer moved, pressed, released, clicked and scrolled, and its keys pressed,
//! released and tapped, text typed and composed, its layout changed.
//!
//! The compositor does what each action asks; this module reads the call, finds the place it
//! names, on one of the seat's outputs or in one of its last captures' images (`captures.rs`
//! keeps them), and tells the result.

use serde_json::{Map, Value, json};
use yuna_layout::{OutputId, SeatId};

use super::Server;
use super::captures::RECENT_CAPTURES;
use super::tools::{self, ButtonCall, KeyboardCall, PointerCall, ToolResult};
use crate::{Error, KeyboardLayout};

impl Server {
    pub(super) fn pointer(&mut self, arguments: Map<String, Value>) -> ToolResult {
        let call: PointerCall = match tools::arguments(arguments) {
            Ok(call) => call,
            Err(reason) => return ToolResult::failed(reason),
        };
        let seat = match &call {
            PointerCall::Get { seat }
            | PointerCall::Move { seat, .. }
            | PointerCall::Scroll { seat, .. } => *seat,
            PointerCall::Press(button)
            | PointerCall::Release(button)
            | PointerCall::Click(button) => button.seat,
        };
        let seat = match self.agent_seat(seat) {
            Ok(seat) => seat,
            Err(failed) => return failed,
        };
        let result = match call {
            PointerCall::Get { .. } => Ok(self.compositor.pointer(seat)),
            PointerCall::Move {
                x,
                y,
                output,
                capture,
                ..
            } => self
                .place(seat, x, y, output, capture)
                .and_then(|(output, x, y)| self.compositor.pointer_move(seat, output, x, y)),
            PointerCall::Press(button) => self.move_first(seat, &button).and_then(|()| {
                self.compositor
                    .pointer_button(seat, button.button.code(), true)
            }),
            PointerCall::Release(button) => self.move_first(seat, &button).and_then(|()| {
                self.compositor
                    .pointer_button(seat, button.button.code(), false)
            }),
            PointerCall::Click(button) => self.move_first(seat, &button).and_then(|()| {
                let code = button.button.code();
                self.compositor.pointer_button(seat, code, true)?;
                self.compositor.pointer_button(seat, code, false)
            }),
            PointerCall::Scroll {
                dx,
                dy,
                x,
                y,
                output,
                capture,
                ..
            } => self
                .move_to(seat, x, y, output, capture)
                .and_then(|()| self.compositor.pointer_scroll(seat, dx, dy)),
        };
        match result {
            Ok(pointer) => ToolResult::done(json!({ "pointer": tools::pointer_json(&pointer) })),
            Err(error) => ToolResult::failed(error.to_string()),
        }
    }

    /// Moves `seat`'s pointer where a button call says, when it says.
    fn move_first(&mut self, seat: SeatId, button: &ButtonCall) -> Result<(), Error> {
        self.move_to(seat, button.x, button.y, button.output, button.capture)
    }

    /// Moves `seat`'s pointer to (`x`, `y`), on `output` or in `capture`'s image, when both are
    /// given; refused for one without the other.
    fn move_to(
        &mut self,
        seat: SeatId,
        x: Option<f64>,
        y: Option<f64>,
        output: Option<u64>,
        capture: Option<u64>,
    ) -> Result<(), Error> {
        match (x, y) {
            (Some(x), Some(y)) => {
                let (output, x, y) = self.place(seat, x, y, output, capture)?;
                self.compositor.pointer_move(seat, output, x, y).map(|_| ())
            }
            (None, None) if output.is_none() && capture.is_none() => Ok(()),
            _ => Err(Error::Refused("give x and y together".to_owned())),
        }
    }

    /// The output and logical place a call's (`x`, `y`) name: on `output`, or, with `capture`,
    /// a point of that capture's image, mapped to its output. Refused for both given, a capture
    /// not among the seat's last [`RECENT_CAPTURES`], and a point off its image.
    fn place(
        &self,
        seat: SeatId,
        x: f64,
        y: f64,
        output: Option<u64>,
        capture: Option<u64>,
    ) -> Result<(Option<OutputId>, f64, f64), Error> {
        let Some(capture) = capture else {
            return Ok((output.map(OutputId), x, y));
        };
        if output.is_some() {
            return Err(Error::Refused(
                "give a capture or an output, not both".to_owned(),
            ));
        }
        let mapping = self
            .recent_captures
            .get(&seat)
            .and_then(|recent| recent.iter().find(|(id, _)| *id == capture))
            .map(|(_, mapping)| *mapping)
            .ok_or_else(|| {
                Error::Refused(format!(
                    "no capture {capture} among seat {}'s last {RECENT_CAPTURES}: capture again",
                    seat.0
                ))
            })?;
        let (logical_x, logical_y) = mapping.to_logical(x, y).ok_or_else(|| {
            Error::Refused(format!(
                "({x}, {y}) is off capture {capture}'s {} × {} pixels",
                mapping.width, mapping.height
            ))
        })?;
        Ok((Some(mapping.output), logical_x, logical_y))
    }

    pub(super) fn keyboard(&mut self, arguments: Map<String, Value>) -> ToolResult {
        let call: KeyboardCall = match tools::arguments(arguments) {
            Ok(call) => call,
            Err(reason) => return ToolResult::failed(reason),
        };
        let seat = match &call {
            KeyboardCall::Get { seat }
            | KeyboardCall::Press { seat, .. }
            | KeyboardCall::Release { seat, .. }
            | KeyboardCall::Tap { seat, .. }
            | KeyboardCall::Type { seat, .. }
            | KeyboardCall::Preedit { seat, .. }
            | KeyboardCall::Layout { seat, .. } => *seat,
        };
        let tells_layout = matches!(call, KeyboardCall::Get { .. } | KeyboardCall::Layout { .. });
        let seat = match self.agent_seat(seat) {
            Ok(seat) => seat,
            Err(failed) => return failed,
        };
        let result = match call {
            KeyboardCall::Get { .. } => Ok(()),
            KeyboardCall::Press { key, .. } => self.compositor.key(seat, &key, true),
            KeyboardCall::Release { key, .. } => self.compositor.key(seat, &key, false),
            KeyboardCall::Tap { keys, .. } => self.compositor.tap_keys(seat, &keys),
            KeyboardCall::Type { text, .. } => self.compositor.type_text(seat, &text),
            KeyboardCall::Preedit { text, .. } => self.compositor.compose_text(seat, &text),
            KeyboardCall::Layout {
                layout,
                variant,
                options,
                ..
            } => {
                let layout = KeyboardLayout {
                    layout,
                    variant: variant.unwrap_or_default(),
                    options: options.unwrap_or_default(),
                };
                self.compositor.set_keyboard_layout(seat, layout)
            }
        };
        if let Err(error) = result {
            return ToolResult::failed(error.to_string());
        }
        let mut told = json!({
            "keyboard_window": self.compositor.keyboard_window(seat).map(|window| window.0),
            "text_field": self.compositor.text_field_enabled(seat),
        });
        if tells_layout && let Some(layout) = self.compositor.keyboard_layout(seat) {
            told["layout"] = json!(layout.layout);
            told["variant"] = json!(layout.variant);
            told["options"] = json!(layout.options);
        }
        ToolResult::done(told)
    }
}
