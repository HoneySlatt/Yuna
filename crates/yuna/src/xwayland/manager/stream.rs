//! The manager's connection's stream: Xwayland's `-wm` socket, whose bytes are followed in X11's
//! framing as they are read, so that a `KeymapNotify` another client sent loses its sent bit
//! before x11rb reads it.
//!
//! **Why.** Every event but `KeymapNotify` carries its sequence number in its bytes 2 and 3.
//! x11rb 0.14 tells a `KeymapNotify` by its first byte alone, 11 (`x11rb-protocol`,
//! `connection/mod.rs`), so that one an X11 client sent with `SendEvent`, whose first byte the
//! server marks with the sent bit, 0x80 | 11, is read as if its bytes 2 and 3 were a sequence
//! number: two bytes the client chose. x11rb then loses its place among the replies, and the
//! manager waits for one for ever. Any X11 client can send one to the manager, which selects
//! SubstructureRedirect on the root. The manager takes no `KeymapNotify`, made up or not: the bit
//! lost changes nothing else it does.
//!
//! **How.** X11's framing from the server is the setup's answer first, 8 bytes and 4 × the
//! length its bytes 6 and 7 give, then packets of 32 bytes, a reply's (1) and a generic event's
//! (35, sent or not) followed by 4 × the length their bytes 4 to 7 give. Lengths are in the
//! connection's byte order, which x11rb chose: its machine's own. [`Framing`] follows that across
//! reads of any size, and clears the bit where a packet starts with 0x80 | 11.

use std::io::{self, IoSlice};
use std::os::fd::{AsFd, BorrowedFd};
use std::sync::{Mutex, PoisonError};

use x11rb::protocol::xproto::{GE_GENERIC_EVENT, KEYMAP_NOTIFY_EVENT};
use x11rb::rust_connection::{DefaultStream, PollMode, RustConnection, Stream};
use x11rb::utils::RawFdContainer;

/// The first byte of a reply.
const REPLY: u8 = 1;

/// The bit of an event's first byte that says a client sent it (X11's protocol, section 10).
const SENT_EVENT: u8 = 0x80;

/// The bytes of the setup's answer before its data.
const SETUP_HEADER: usize = 8;

/// The bytes of every packet after the setup: an error, a reply's start, an event.
const PACKET: usize = 32;

/// The manager's connection to Xwayland, over [`ManagerStream`].
pub(crate) type ManagerConnection = RustConnection<ManagerStream>;

/// Xwayland's `-wm` socket, its bytes followed as they are read ([`Framing`]).
#[derive(Debug)]
pub(crate) struct ManagerStream {
    inner: DefaultStream,
    /// Held across each read and the following of its bytes: two reads would otherwise be
    /// followed out of their order.
    framing: Mutex<Framing>,
}

impl ManagerStream {
    pub(super) fn new(inner: DefaultStream) -> ManagerStream {
        ManagerStream {
            inner,
            framing: Mutex::new(Framing::default()),
        }
    }
}

impl Stream for ManagerStream {
    fn poll(&self, mode: PollMode) -> io::Result<()> {
        self.inner.poll(mode)
    }

    fn read(&self, buf: &mut [u8], fd_storage: &mut Vec<RawFdContainer>) -> io::Result<usize> {
        // Its state is whole between reads: a panic elsewhere leaves it as it was.
        let mut framing = self.framing.lock().unwrap_or_else(PoisonError::into_inner);
        let read = self.inner.read(buf, fd_storage)?;
        framing.follow(&mut buf[..read]);
        Ok(read)
    }

    fn write(&self, buf: &[u8], fds: &mut Vec<RawFdContainer>) -> io::Result<usize> {
        self.inner.write(buf, fds)
    }

    fn write_vectored(
        &self,
        bufs: &[IoSlice<'_>],
        fds: &mut Vec<RawFdContainer>,
    ) -> io::Result<usize> {
        self.inner.write_vectored(bufs, fds)
    }
}

impl AsFd for ManagerStream {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.inner.as_fd()
    }
}

/// Where the bytes read so far stand in X11's packets from the server.
#[derive(Debug, Default)]
struct Framing {
    /// Whether the setup's answer was passed: packets follow.
    set_up: bool,
    /// The current header's bytes read so far: the setup's 8, or a packet's first 32.
    header: [u8; PACKET],
    filled: usize,
    /// The bytes still to come past the current header, which nothing reads.
    body: usize,
}

impl Framing {
    /// Follows `bytes`, the next read, and clears the sent bit of each `KeymapNotify` sent by a
    /// client that starts in it.
    fn follow(&mut self, bytes: &mut [u8]) {
        let mut at = 0;
        while at < bytes.len() {
            if self.body > 0 {
                let passed = self.body.min(bytes.len() - at);
                self.body -= passed;
                at += passed;
                continue;
            }
            if self.set_up && self.filled == 0 && bytes[at] == SENT_EVENT | KEYMAP_NOTIFY_EVENT {
                bytes[at] = KEYMAP_NOTIFY_EVENT;
            }
            let length = if self.set_up { PACKET } else { SETUP_HEADER };
            let taken = (length - self.filled).min(bytes.len() - at);
            self.header[self.filled..self.filled + taken].copy_from_slice(&bytes[at..at + taken]);
            self.filled += taken;
            at += taken;
            if self.filled == length {
                self.body = self.body_after_header();
                self.filled = 0;
                self.set_up = true;
            }
        }
    }

    /// The bytes that follow the header just read: the setup's data, a reply's or a generic
    /// event's rest, or none.
    fn body_after_header(&self) -> usize {
        let header = &self.header;
        let units = if !self.set_up {
            u32::from(u16::from_ne_bytes([header[6], header[7]]))
        } else if header[0] == REPLY || header[0] & !SENT_EVENT == GE_GENERIC_EVENT {
            u32::from_ne_bytes([header[4], header[5], header[6], header[7]])
        } else {
            0
        };
        // At most 16 GiB, which a 64-bit usize holds; a smaller one stops at its largest.
        usize::try_from(u64::from(units) * 4).unwrap_or(usize::MAX)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SENT_KEYMAP: u8 = SENT_EVENT | KEYMAP_NOTIFY_EVENT;

    /// A setup's answer whose data, of `units` × 4 bytes, are each `fill`.
    fn setup(units: u16, fill: u8) -> Vec<u8> {
        let mut bytes = vec![1, 0, 11, 0, 0, 0];
        bytes.extend_from_slice(&units.to_ne_bytes());
        bytes.resize(SETUP_HEADER + usize::from(units) * 4, fill);
        bytes
    }

    /// A packet of 32 bytes starting with `first`, the others `fill`.
    fn packet(first: u8, fill: u8) -> Vec<u8> {
        let mut bytes = vec![fill; PACKET];
        bytes[0] = first;
        bytes
    }

    /// A reply, or with `first` 35 a generic event, of 32 bytes and `units` × 4 more, each
    /// byte past its length `fill`.
    fn long_packet(first: u8, units: u32, fill: u8) -> Vec<u8> {
        let mut bytes = vec![first, 0, 0, 0];
        bytes.extend_from_slice(&units.to_ne_bytes());
        bytes.resize(PACKET + units as usize * 4, fill);
        bytes
    }

    /// `stream` read in pieces cut at `cuts`, followed by a new framing.
    fn followed_in_pieces(stream: &[u8], cuts: &[usize]) -> Vec<u8> {
        let mut framing = Framing::default();
        let mut bytes = stream.to_vec();
        let mut start = 0;
        for &cut in cuts.iter().chain([&stream.len()]) {
            framing.follow(&mut bytes[start..cut]);
            start = cut;
        }
        bytes
    }

    #[test]
    fn a_sent_keymap_notify_loses_its_sent_bit_wherever_the_reads_cut() {
        // The setup's data, a reply and a generic event are full of the same byte: none of them
        // is an event's start.
        let mut stream = setup(9, SENT_KEYMAP);
        stream.extend(long_packet(REPLY, 3, SENT_KEYMAP));
        let keymap_at = stream.len();
        stream.extend(packet(SENT_KEYMAP, 0x55));
        stream.extend(long_packet(SENT_EVENT | GE_GENERIC_EVENT, 2, SENT_KEYMAP));
        stream.extend(long_packet(GE_GENERIC_EVENT, 1, SENT_KEYMAP));
        let second_at = stream.len();
        stream.extend(packet(SENT_KEYMAP, SENT_KEYMAP));
        // An error, then a KeymapNotify of the server's own, unchanged.
        stream.extend(packet(0, SENT_KEYMAP));
        stream.extend(packet(KEYMAP_NOTIFY_EVENT, SENT_KEYMAP));
        let mut expected = stream.clone();
        expected[keymap_at] = KEYMAP_NOTIFY_EVENT;
        expected[second_at] = KEYMAP_NOTIFY_EVENT;
        assert_eq!(followed_in_pieces(&stream, &[]), expected);
        for cut in 1..stream.len() {
            assert_eq!(
                followed_in_pieces(&stream, &[cut]),
                expected,
                "read in two at {cut}"
            );
        }
        let every_byte: Vec<usize> = (1..stream.len()).collect();
        assert_eq!(followed_in_pieces(&stream, &every_byte), expected);
    }

    #[test]
    fn other_events_and_replies_are_left_whole() {
        let mut stream = setup(0, 0);
        for first in [2, 12, SENT_EVENT | 33, SENT_EVENT | 31, 0, REPLY] {
            stream.extend(packet(first, SENT_KEYMAP));
        }
        assert_eq!(followed_in_pieces(&stream, &[3, 40, 77]), stream);
    }
}
