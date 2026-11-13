//! MCP's stdio framing: one JSON-RPC message per line, read from the client and written back
//! ([survey](../../../../docs/survey.md#agents-and-mcp)). Bounded both ways: a line over
//! [`MAX_LINE`] ends the connection, and replies wait in a queue whose length the server reads to
//! stop reading requests while the client does not read its replies.

use std::io;
use std::os::fd::AsFd;

use serde_json::Value;
use smithay::reexports::rustix;

/// The longest line accepted, newline excluded: 1 MiB. A request is a few hundred bytes; a line
/// past this is a client gone wrong, and ends the connection.
pub const MAX_LINE: usize = 1 << 20;

/// A line that passed [`MAX_LINE`] without its newline.
#[derive(Debug, PartialEq, Eq)]
pub struct LineTooLong;

/// Cuts the bytes read into lines.
#[derive(Debug, Default)]
pub struct LineReader {
    /// What was read and is not yet a whole line.
    partial: Vec<u8>,
}

impl LineReader {
    /// Adds `bytes` and appends each line they complete to `lines`, without its `\n` and a `\r`
    /// before it. Empty lines are skipped. Fails once the line being read passes [`MAX_LINE`].
    pub fn push(&mut self, bytes: &[u8], lines: &mut Vec<Vec<u8>>) -> Result<(), LineTooLong> {
        let mut rest = bytes;
        while let Some(end) = rest.iter().position(|&byte| byte == b'\n') {
            self.partial.extend_from_slice(&rest[..end]);
            rest = &rest[end + 1..];
            let mut line = std::mem::take(&mut self.partial);
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            if line.len() > MAX_LINE {
                return Err(LineTooLong);
            }
            if !line.is_empty() {
                lines.push(line);
            }
        }
        self.partial.extend_from_slice(rest);
        if self.partial.len() > MAX_LINE {
            return Err(LineTooLong);
        }
        Ok(())
    }

    /// Whether part of a line waits for its newline.
    pub fn has_partial_line(&self) -> bool {
        !self.partial.is_empty()
    }
}

/// The replies waiting to be written, in order.
#[derive(Debug, Default)]
pub struct OutputQueue {
    bytes: Vec<u8>,
    /// How much of `bytes` is written already.
    written: usize,
}

impl OutputQueue {
    /// Queues `message` as one line. serde_json writes no raw newline in compact form: those in
    /// strings are escaped.
    pub fn push(&mut self, message: &Value) {
        // A `Value` always serialises: its keys are strings.
        if serde_json::to_writer(&mut self.bytes, message).is_ok() {
            self.bytes.push(b'\n');
        }
    }

    /// Bytes waiting to be written.
    pub fn len(&self) -> usize {
        self.bytes.len() - self.written
    }

    /// Whether nothing waits.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Writes what waits to `fd`, which must not block, until it is all written or `fd` is full.
    /// Fails when the reader has gone.
    pub fn write_to(&mut self, fd: impl AsFd) -> io::Result<()> {
        while self.written < self.bytes.len() {
            match rustix::io::write(&fd, &self.bytes[self.written..]) {
                Ok(0) => return Err(io::ErrorKind::WriteZero.into()),
                Ok(count) => self.written += count,
                Err(rustix::io::Errno::AGAIN) => break,
                Err(rustix::io::Errno::INTR) => continue,
                Err(error) => return Err(error.into()),
            }
        }
        if self.written == self.bytes.len() {
            self.bytes.clear();
            self.written = 0;
        } else if self.written > self.bytes.len() / 2 {
            // Keeps the buffer from growing with what was written already.
            self.bytes.drain(..self.written);
            self.written = 0;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn lines_of(chunks: &[&[u8]]) -> (Vec<Vec<u8>>, LineReader) {
        let mut reader = LineReader::default();
        let mut lines = Vec::new();
        for chunk in chunks {
            reader.push(chunk, &mut lines).expect("short lines");
        }
        (lines, reader)
    }

    #[test]
    fn lines_are_cut_across_reads_without_their_ends() {
        let (lines, reader) = lines_of(&[b"{\"a\"", b":1}\n{\"b\":2}\r\n\n{\"c\""]);
        assert_eq!(lines, vec![b"{\"a\":1}".to_vec(), b"{\"b\":2}".to_vec()]);
        assert!(reader.has_partial_line());
    }

    #[test]
    fn a_line_over_the_limit_fails_with_or_without_its_newline() {
        let mut reader = LineReader::default();
        let mut lines = Vec::new();
        let long = vec![b'x'; MAX_LINE + 1];
        assert_eq!(reader.push(&long, &mut lines), Err(LineTooLong));
        let mut reader = LineReader::default();
        let mut exact = vec![b'x'; MAX_LINE];
        exact.push(b'\n');
        assert_eq!(reader.push(&exact, &mut lines), Ok(()));
        assert_eq!(lines.len(), 1);
        let mut reader = LineReader::default();
        assert_eq!(reader.push(&vec![b'x'; MAX_LINE], &mut lines), Ok(()));
        assert_eq!(reader.push(b"x\n", &mut lines), Err(LineTooLong));
    }

    #[test]
    fn a_queued_message_is_one_line_even_with_newlines_in_its_strings() {
        let mut queue = OutputQueue::default();
        queue.push(&json!({"text": "two\nlines"}));
        let (ours, theirs) = std::os::unix::net::UnixStream::pair().expect("a socket pair");
        queue.write_to(&ours).expect("written");
        assert!(queue.is_empty());
        drop(ours);
        let mut written = String::new();
        io::Read::read_to_string(&mut &theirs, &mut written).expect("read");
        assert_eq!(written, "{\"text\":\"two\\nlines\"}\n");
    }

    #[test]
    fn a_full_reader_keeps_the_rest_queued_and_in_order() {
        let (ours, theirs) = std::os::unix::net::UnixStream::pair().expect("a socket pair");
        ours.set_nonblocking(true).expect("non-blocking");
        let mut queue = OutputQueue::default();
        let big = "y".repeat(64 * 1024);
        for index in 0..64 {
            queue.push(&json!({"index": index, "text": big}));
        }
        queue.write_to(&ours).expect("partly written");
        assert!(!queue.is_empty(), "a socket's buffer is smaller than 4 MiB");
        let reader = std::thread::spawn(move || {
            let mut all = Vec::new();
            io::Read::read_to_end(&mut &theirs, &mut all).expect("read");
            all
        });
        while !queue.is_empty() {
            queue.write_to(&ours).expect("written");
            std::thread::yield_now();
        }
        drop(ours);
        let all = reader.join().expect("the reader");
        let indices: Vec<u64> = all
            .split(|&byte| byte == b'\n')
            .filter(|line| !line.is_empty())
            .map(|line| {
                let value: Value = serde_json::from_slice(line).expect("a JSON line");
                value["index"].as_u64().expect("an index")
            })
            .collect();
        assert_eq!(indices, (0..64).collect::<Vec<u64>>());
    }
}
