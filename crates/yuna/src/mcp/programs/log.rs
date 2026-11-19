//! A program's log, and how its output reaches it: [`ProgramLog`], a file that keeps the first
//! [`LOG_HEAD`] bytes of the output and a ring of its newest [`LOG_TAIL`], read by offsets since
//! the program started ([`LogRead`]); and the source that reads the output into it, throttled
//! once the program has written more than its log keeps ([`THROTTLE`]).
//!
//! Its neighbours: `mod.rs` gives each program a log, and a seat's `dbus-daemon` one
//! (`Programs::bounded_log`); the server lets the throttled sources read again (`../spawn.rs`).

use std::cell::{Cell, RefCell};
use std::fs::File;
use std::io;
use std::os::fd::{AsFd, OwnedFd};
use std::os::unix::fs::FileExt;
use std::rc::Rc;
use std::time::Duration;

use smithay::reexports::calloop::generic::Generic;
use smithay::reexports::calloop::{Interest, LoopHandle, Mode, PostAction, RegistrationToken};

use crate::rest::{Wake, insert_source};
use crate::state::State;

/// How much of the start of a program's output its log keeps: 256 KiB, where a failure to start
/// shows.
pub const LOG_HEAD: u64 = 256 << 10;

/// How much of the newest of a program's output its log keeps, after its first [`LOG_HEAD`]:
/// 1 MiB. What comes between is counted and dropped, so that a program that never stops writing
/// fills neither the disk nor memory: the logs are in `XDG_RUNTIME_DIR`, most often in memory,
/// [`MAX_KEPT`](super::MAX_KEPT) of them at most, 160 MiB together at worst.
pub const LOG_TAIL: u64 = 1 << 20;

/// How much of a program's output is read at a time.
const READ_SIZE: usize = 64 * 1024;

/// How often a program's output is read once it has written more than its log keeps.
pub const THROTTLE: Duration = Duration::from_millis(100);

/// A read of a program's log, as `spawn log` gives it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogRead {
    /// Bytes `from` to `to` of its output, as UTF-8, any invalid sequence replaced.
    pub text: String,
    /// Where the bytes start and end, in bytes since the program started.
    pub from: u64,
    pub to: u64,
    /// Every byte it wrote.
    pub size: u64,
    /// The bytes its log did not keep.
    pub dropped: u64,
    /// Whether its output has ended: every process that held its pipe has closed it.
    pub ended: bool,
}

/// A program's log: its file, which holds its first [`LOG_HEAD`] bytes, then its newest
/// [`LOG_TAIL`] in a ring, byte `n` past the head at `LOG_HEAD + (n - LOG_HEAD) % LOG_TAIL`.
#[derive(Debug)]
pub(super) struct ProgramLog {
    file: File,
    /// Every byte the program wrote.
    pub(super) size: u64,
    /// The bytes put in the file: `size`, unless a write failed, after which none is.
    written: u64,
    failed: bool,
    pub(super) ended: bool,
    /// Rounds in which its output woke Yuna.
    pub(super) wakes: u64,
}

impl ProgramLog {
    pub(super) fn new(file: File) -> ProgramLog {
        ProgramLog {
            file,
            size: 0,
            written: 0,
            failed: false,
            ended: false,
            wakes: 0,
        }
    }

    /// Where byte `offset` of the output is in the file.
    fn position(offset: u64) -> u64 {
        if offset < LOG_HEAD {
            offset
        } else {
            LOG_HEAD + (offset - LOG_HEAD) % LOG_TAIL
        }
    }

    /// How many bytes from byte `offset` lie in the file before its head's end or the ring's
    /// end, whichever comes first.
    fn contiguous_from(offset: u64) -> u64 {
        if offset < LOG_HEAD {
            LOG_HEAD - offset
        } else {
            LOG_HEAD + LOG_TAIL - Self::position(offset)
        }
    }

    /// The end of the head kept, and the start of the tail kept: equal when nothing was
    /// dropped between.
    fn kept_bounds(&self) -> (u64, u64) {
        let head_end = self.written.min(LOG_HEAD);
        let tail_start = head_end.max(self.written.saturating_sub(LOG_TAIL));
        (head_end, tail_start)
    }

    pub(super) fn kept(&self) -> u64 {
        let (head_end, tail_start) = self.kept_bounds();
        head_end + (self.written - tail_start)
    }

    pub(super) fn dropped(&self) -> u64 {
        self.size - self.kept()
    }

    /// Adds `bytes` the program wrote. A write that fails, the disk full most often, is said
    /// once, and nothing more goes into the file: what came after is dropped.
    pub(super) fn append(&mut self, bytes: &[u8]) {
        self.size += bytes.len() as u64;
        let mut rest = bytes;
        while !rest.is_empty() && !self.failed {
            let room = usize::try_from(Self::contiguous_from(self.written)).unwrap_or(usize::MAX);
            let length = room.min(rest.len());
            match self
                .file
                .write_all_at(&rest[..length], Self::position(self.written))
            {
                Ok(()) => {
                    self.written += length as u64;
                    rest = &rest[length..];
                }
                Err(error) => {
                    tracing::warn!(%error, "a program's log cannot be written: the rest is dropped");
                    self.failed = true;
                }
            }
        }
    }

    /// Up to `max_bytes` of the bytes kept from byte `from`, or the newest without. A start in
    /// the bytes dropped moves to the first kept after; the bytes given never cross what was
    /// dropped, so that they are whole: the next read starts at their end.
    pub(super) fn read(&self, from: Option<u64>, max_bytes: u64) -> Result<LogRead, String> {
        let (head_end, tail_start) = self.kept_bounds();
        let mut start = from
            .unwrap_or_else(|| self.written.saturating_sub(max_bytes))
            .min(self.written);
        if (head_end..tail_start).contains(&start) {
            start = tail_start;
        }
        let limit = if start < head_end && head_end < tail_start {
            head_end
        } else {
            self.written
        };
        let end = start.saturating_add(max_bytes).min(limit);
        let mut bytes = vec![0; usize::try_from(end - start).unwrap_or(0)];
        let mut offset = start;
        let mut filled = 0;
        while offset < end {
            let length = Self::contiguous_from(offset).min(end - offset) as usize;
            self.file
                .read_exact_at(&mut bytes[filled..filled + length], Self::position(offset))
                .map_err(|error| format!("its log cannot be read: {error}"))?;
            filled += length;
            offset += length as u64;
        }
        Ok(LogRead {
            text: String::from_utf8_lossy(&bytes).into_owned(),
            from: start,
            to: end,
            size: self.size,
            dropped: self.dropped(),
            ended: self.ended,
        })
    }
}

/// The tokens of the output sources waiting for the end of a [`THROTTLE`]. A source holds this
/// list, never the loop's handle: a source holding the handle would keep the loop, and Yuna's
/// socket with it, alive for ever.
pub(super) type Throttled = Rc<RefCell<Vec<Rc<Cell<Option<RegistrationToken>>>>>>;

/// Reads a program's output as it comes, into `log`, the source named `wake`; the source goes
/// when the output ends.
/// Once the program has written more than its log keeps, [`LOG_HEAD`] and [`LOG_TAIL`], it is
/// read at most [`READ_SIZE`] every [`THROTTLE`]: the source stops reading and joins
/// `throttled`, which the server lets read again at the end of the throttle. A program that
/// writes faster waits on its pipe, rather than keep Yuna reading for what it drops.
pub(super) fn watch_output(
    handle: &LoopHandle<'static, State>,
    wake: Wake,
    output: OwnedFd,
    log: Rc<RefCell<ProgramLog>>,
    throttled: Throttled,
) -> io::Result<()> {
    rustix::io::ioctl_fionbio(&output, true)?;
    let mut buffer = vec![0; READ_SIZE];
    let source: Rc<Cell<Option<RegistrationToken>>> = Rc::default();
    let own_source = source.clone();
    let token = insert_source(
        handle,
        wake,
        Generic::new(output, Interest::READ, Mode::Level),
        move |_, output, _| {
            log.borrow_mut().wakes += 1;
            match rustix::io::read(output.as_fd(), &mut buffer) {
                Ok(0) => {
                    log.borrow_mut().ended = true;
                    Ok(PostAction::Remove)
                }
                Ok(read) => {
                    let mut log = log.borrow_mut();
                    log.append(&buffer[..read]);
                    if log.size < LOG_HEAD + LOG_TAIL {
                        return Ok(PostAction::Continue);
                    }
                    throttled.borrow_mut().push(own_source.clone());
                    Ok(PostAction::Disable)
                }
                Err(rustix::io::Errno::AGAIN | rustix::io::Errno::INTR) => Ok(PostAction::Continue),
                Err(error) => {
                    tracing::warn!(%error, "a program's output cannot be read");
                    log.borrow_mut().ended = true;
                    Ok(PostAction::Remove)
                }
            }
        },
    )
    .map_err(|e| io::Error::other(e.error.to_string()))?;
    source.set(Some(token));
    Ok(())
}
