//! The `clipboard` tool: a seat's clipboard or primary selection read, set and cleared
//! ([ADR-0101](../../../../docs/adr/0101-an-agents-clipboard-through-mcp.md)).
//!
//! A `clipboard get` waits for the program that copied to write into a pipe, which each round
//! reads as far as it goes, never waiting, and is answered once the program closes it, past its
//! `max_bytes`, or at [`CLIPBOARD_TIMEOUT`] with what came. It counts among the calls that wait
//! (`waits.rs`). The selections themselves, and the text Yuna holds for an agent, are the
//! compositor's (`crate::selection`).

use std::os::fd::OwnedFd;
use std::sync::Arc;

use serde_json::{Map, Value, json};
use smithay::reexports::calloop::generic::Generic;
use smithay::reexports::calloop::timer::{TimeoutAction, Timer};
use smithay::reexports::calloop::{Interest, Mode, PostAction, RegistrationToken};
use yuna_layout::SeatId;

use super::Server;
use super::protocol::Revision;
use super::tools::{
    self, ClipboardCall, ClipboardData, MAX_CLIPBOARD_TEXT_BYTES, MAX_WAITING_CALLS, ToolResult,
};
use crate::CLIPBOARD_TIMEOUT;
use crate::rest::{Source, Wake, insert_source};
use crate::selection::SelectionRead;
use crate::xwayland::Selection;

/// A `clipboard get` reading its selection from the program that copied it, until it is
/// answered.
#[derive(Debug)]
pub(super) struct ClipboardRead {
    pub(super) key: u64,
    pub(super) id: Value,
    pub(super) revision: Revision,
    pub(super) seat: SeatId,
    /// The MIME types the selection offered, which the answer gives.
    mime_types: Vec<String>,
    /// The type read.
    mime: String,
    max_bytes: usize,
    /// The read end of the pipe the program writes into, non-blocking.
    pipe: OwnedFd,
    /// What came, up to one byte past `max_bytes`, which tells that the program had more.
    data: Vec<u8>,
    /// The source that wakes the loop when the pipe may be read, on a copy of it.
    source: RegistrationToken,
    /// The timer that answers it [`CLIPBOARD_TIMEOUT`] after it started.
    timer: RegistrationToken,
}

/// How a `clipboard get`'s read ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ReadEnd {
    /// The program closed its pipe: the data came whole.
    Closed,
    /// It wrote more than `max_bytes`.
    Truncated,
    /// Its time is up, with what came.
    TimedOut,
    /// The pipe failed.
    Failed(rustix::io::Errno),
}

impl Server {
    /// `clipboard`: the result now, or none while a `get` waits for its selection's data.
    pub(super) fn clipboard(
        &mut self,
        id: &Value,
        revision: Revision,
        arguments: Map<String, Value>,
    ) -> Option<ToolResult> {
        let call: ClipboardCall = match tools::arguments(arguments) {
            Ok(call) => call,
            Err(reason) => return Some(ToolResult::failed(reason)),
        };
        let seat = match self.agent_seat(call.seat()) {
            Ok(seat) => seat,
            Err(failed) => return Some(failed),
        };
        let selection = call.selection();
        match call {
            ClipboardCall::Get {
                mime, max_bytes, ..
            } => match tools::clipboard_bytes(max_bytes) {
                Ok(max_bytes) => self.clipboard_get(id, revision, seat, selection, mime, max_bytes),
                Err(reason) => Some(ToolResult::failed(reason)),
            },
            ClipboardCall::Set { text, .. } => {
                if text.len() > MAX_CLIPBOARD_TEXT_BYTES {
                    return Some(ToolResult::failed(format!(
                        "the text takes {} bytes, over the {MAX_CLIPBOARD_TEXT_BYTES} set takes",
                        text.len()
                    )));
                }
                let bytes = text.len();
                let text: Arc<[u8]> = Arc::from(text.into_bytes());
                self.compositor.hold_agent_selection(seat, selection, text);
                Some(ToolResult::done(json!({
                    "mime_types": self.compositor.selection_mime_types(seat, selection),
                    "bytes": bytes,
                })))
            }
            ClipboardCall::Clear { .. } => {
                self.compositor.clear_selection(seat, selection);
                Some(ToolResult::done(json!({ "mime_types": [] })))
            }
        }
    }

    /// `clipboard get` of `seat`'s `selection`, as `mime` or its first text type: the answer
    /// now when the selection is empty, offers no text type, or Yuna holds its text; else none,
    /// while its pipe is read, counted among the waiting calls.
    fn clipboard_get(
        &mut self,
        id: &Value,
        revision: Revision,
        seat: SeatId,
        selection: Selection,
        mime: Option<String>,
        max_bytes: usize,
    ) -> Option<ToolResult> {
        let mime_types = self.compositor.selection_mime_types(seat, selection);
        if mime_types.is_empty() {
            return Some(ToolResult::done(tools::clipboard_json(&[], None)));
        }
        let mime = match mime {
            Some(mime) if mime_types.contains(&mime) => mime,
            Some(mime) => {
                return Some(ToolResult::failed(format!(
                    "the selection offers no {mime}: it offers {}",
                    mime_types.join(", ")
                )));
            }
            None => match tools::first_text_type(&mime_types) {
                Some(text) => text,
                None => return Some(ToolResult::done(tools::clipboard_json(&mime_types, None))),
            },
        };
        // Before the source is asked, so that none is asked in vain.
        if self.waiting_calls() >= MAX_WAITING_CALLS {
            return Some(ToolResult::failed(format!(
                "{MAX_WAITING_CALLS} calls wait already"
            )));
        }
        let pipe = match self
            .compositor
            .read_selection(seat, selection, mime.clone())
        {
            Ok(SelectionRead::Pipe(pipe)) => pipe,
            Ok(SelectionRead::Empty) => {
                return Some(ToolResult::done(tools::clipboard_json(&[], None)));
            }
            Ok(SelectionRead::Held(text)) => {
                let data = ClipboardData {
                    mime,
                    bytes: text[..text.len().min(max_bytes)].to_vec(),
                    truncated: text.len() > max_bytes,
                    timed_out: false,
                };
                let answer = tools::clipboard_json(&mime_types, Some(&data));
                return Some(ToolResult::done(answer));
            }
            Err(error) => return Some(ToolResult::failed(error.to_string())),
        };
        let key = self.next_wait;
        let (source, timer) = match self.watch_clipboard_read(key, seat, &pipe) {
            Ok(tokens) => tokens,
            Err(error) => return Some(ToolResult::failed(error)),
        };
        self.next_wait += 1;
        self.clipboard_reads.push(ClipboardRead {
            key,
            id: id.clone(),
            revision,
            seat,
            mime_types,
            mime,
            max_bytes,
            pipe,
            data: Vec::new(),
            source,
            timer,
        });
        None
    }

    /// The sources of clipboard read `key`, of `seat`: one noting when `pipe` may be read, and
    /// the timer of [`CLIPBOARD_TIMEOUT`]. Neither is left when one cannot be made.
    fn watch_clipboard_read(
        &self,
        key: u64,
        seat: SeatId,
        pipe: &OwnedFd,
    ) -> Result<(RegistrationToken, RegistrationToken), String> {
        let handle = self.compositor.loop_handle();
        let watched = pipe.try_clone().map_err(|error| error.to_string())?;
        let events = self.events.clone();
        let source = insert_source(
            &handle,
            Wake::of_seat(Source::ClipboardRead, seat),
            Generic::new(watched, Interest::READ, Mode::Level),
            move |_, _, _| {
                let readable = &mut events.borrow_mut().clipboards_readable;
                if !readable.contains(&key) {
                    readable.push(key);
                }
                Ok(PostAction::Continue)
            },
        )
        .map_err(|error| error.error.to_string())?;
        let events = self.events.clone();
        let timer = insert_source(
            &handle,
            Wake::of_seat(Source::ClipboardReadTimeout, seat),
            Timer::from_duration(CLIPBOARD_TIMEOUT),
            move |_, _, _| {
                events.borrow_mut().timed_out.push(key);
                TimeoutAction::Drop
            },
        );
        match timer {
            Ok(timer) => Ok((source, timer)),
            Err(error) => {
                handle.remove(source);
                Err(error.error.to_string())
            }
        }
    }

    /// Reads what the pipes of the clipboard reads `keys` hold now, and answers each that ended.
    pub(super) fn read_clipboards(&mut self, keys: Vec<u64>) {
        for key in keys {
            let Some(index) = self.clipboard_reads.iter().position(|read| read.key == key) else {
                continue;
            };
            if let Some(end) = self.drain_clipboard_read(index) {
                self.answer_clipboard_read(index, end);
            }
        }
    }

    /// Reads what the pipe of the clipboard read at `index` holds now, never waiting: how the
    /// read ended, or none while the program may write more.
    pub(super) fn drain_clipboard_read(&mut self, index: usize) -> Option<ReadEnd> {
        let read = &mut self.clipboard_reads[index];
        let buffer = &mut self.read_buffer;
        loop {
            // One byte past max_bytes tells that the program had more.
            let room = (read.max_bytes + 1 - read.data.len()).min(buffer.len());
            match rustix::io::read(&read.pipe, &mut buffer[..room]) {
                Ok(0) => return Some(ReadEnd::Closed),
                Ok(count) => {
                    read.data.extend_from_slice(&buffer[..count]);
                    if read.data.len() > read.max_bytes {
                        return Some(ReadEnd::Truncated);
                    }
                }
                Err(rustix::io::Errno::AGAIN) => return None,
                Err(rustix::io::Errno::INTR) => {}
                Err(error) => return Some(ReadEnd::Failed(error)),
            }
        }
    }

    /// Answers the clipboard read at `index`, which ended so; its pipe closes, and a program
    /// still writing sees it closed.
    pub(super) fn answer_clipboard_read(&mut self, index: usize, end: ReadEnd) {
        let read = self.drop_clipboard_read(index);
        let result = match end {
            ReadEnd::Failed(error) => {
                ToolResult::failed(format!("the selection could not be read: {error}"))
            }
            ReadEnd::Closed | ReadEnd::Truncated | ReadEnd::TimedOut => {
                let mut bytes = read.data;
                bytes.truncate(read.max_bytes);
                let data = ClipboardData {
                    mime: read.mime,
                    bytes,
                    truncated: end == ReadEnd::Truncated,
                    timed_out: end == ReadEnd::TimedOut,
                };
                ToolResult::done(tools::clipboard_json(&read.mime_types, Some(&data)))
            }
        };
        self.answer_waited(read.id, read.revision, result);
    }

    /// Takes the clipboard read at `index` out, unanswered, its sources removed.
    pub(super) fn drop_clipboard_read(&mut self, index: usize) -> ClipboardRead {
        let read = self.clipboard_reads.remove(index);
        let handle = self.compositor.loop_handle();
        handle.remove(read.source);
        // A timer that fired has dropped itself: its token is then stale, and nothing happens.
        handle.remove(read.timer);
        read
    }
}
