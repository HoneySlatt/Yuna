//! What the bench reads from the compositors' and the clients' own logs and counters: the frames
//! each compositor drew in a window, `weston-presentation-shm`'s presentation log, and mpv's
//! counters of dropped and delayed frames over its IPC.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::Duration;

/// The value at percentile `p` of `sorted`, by the nearest rank; none if it is empty.
pub fn percentile(sorted: &[f64], p: f64) -> Option<f64> {
    if sorted.is_empty() {
        return None;
    }
    let rank = ((p / 100.0) * sorted.len() as f64).ceil() as usize;
    Some(sorted[rank.clamp(1, sorted.len()) - 1])
}

/// KWin's frames in `[from_ns, to_ns]` of CLOCK_MONOTONIC, from its `KWIN_LOG_PERFORMANCE_DATA`
/// CSV (`src/core/renderloop.cpp`): a row per frame presented, its second column the time it was
/// presented. Gives the frames and the median of the sixth column, the refresh's length in ns.
pub fn kwin_frames(csv: &str, from_ns: u64, to_ns: u64) -> (u64, Option<f64>) {
    let mut frames = 0;
    let mut refreshes = Vec::new();
    for line in csv.lines().skip(1) {
        let columns: Vec<&str> = line.split(',').collect();
        let (Some(presented), Some(refresh)) = (
            columns.get(1).and_then(|c| c.trim().parse::<u64>().ok()),
            columns.get(5).and_then(|c| c.trim().parse::<f64>().ok()),
        ) else {
            continue;
        };
        if (from_ns..=to_ns).contains(&presented) {
            frames += 1;
            refreshes.push(refresh);
        }
    }
    refreshes.sort_by(f64::total_cmp);
    (frames, percentile(&refreshes, 50.0))
}

/// The output Yuna made, from its log line `virtual output created output=1 width=3840
/// height=2160 millihertz=240000 scale=1.5`, as `3840x2160@240.000,1.5`.
pub fn yuna_output(log: &str) -> Option<String> {
    let line = log
        .lines()
        .find(|line| line.contains("virtual output created"))?;
    let field = |key: &str| {
        line.split_whitespace()
            .find_map(|word| word.strip_prefix(key)?.strip_prefix('='))
    };
    let millihertz: f64 = field("millihertz")?.parse().ok()?;
    Some(format!(
        "{}x{}@{:.3},{}",
        field("width")?,
        field("height")?,
        millihertz / 1000.0,
        field("scale")?
    ))
}

/// The frame counter's frames in `[from_ns, to_ns]`: its `frame <ns>` lines.
pub fn counted_frames(log: &str, from_ns: u64, to_ns: u64) -> u64 {
    log.lines()
        .filter_map(|line| line.strip_prefix("frame ")?.trim().parse::<u64>().ok())
        .filter(|ns| (from_ns..=to_ns).contains(ns))
        .count() as u64
}

/// What `weston-presentation-shm`'s feedback mode printed: a line per frame presented,
/// `<n>: f2c <ms> ms, c2p <ms> ms, f2p <ms> ms, p2p <us> us, t2p <us>, [<flags>], seq <msc>`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PresentationLog {
    /// Each frame's time from the previous presentation to its own, `p2p`, in µs.
    pub between_us: Vec<i64>,
    /// The frames it saw discarded.
    pub discarded: u64,
    /// The lines that start as a frame's, `<n>:`, but whose `p2p` could not be read.
    pub unread: u64,
}

/// Reads a stretch of the presentation log.
pub fn parse_presentation_log(log: &str) -> PresentationLog {
    let mut read = PresentationLog::default();
    for line in log.lines().map(str::trim) {
        if line.starts_with("discarded ") {
            read.discarded += 1;
            continue;
        }
        let Some((number, rest)) = line.split_once(':') else {
            continue;
        };
        if number.is_empty() || !number.chars().all(|c| c.is_ascii_digit()) {
            continue;
        }
        let between = rest
            .split(',')
            .map(str::trim)
            .find_map(|part| part.strip_prefix("p2p "))
            .and_then(|value| value.split_whitespace().next()?.parse().ok());
        match between {
            Some(us) => read.between_us.push(us),
            None => read.unread += 1,
        }
    }
    read
}

/// The properties of mpv read over its IPC: its counters of frames dropped and delayed.
pub const MPV_COUNTERS: [&str; 4] = [
    "frame-drop-count",
    "decoder-frame-drop-count",
    "vo-delayed-frame-count",
    "mistimed-frame-count",
];

/// Reads property `name` of the mpv whose IPC socket is `socket`, as a number.
pub fn mpv_property(socket: &Path, name: &str) -> Result<Option<f64>, String> {
    mpv_value(socket, name).map(|value| value.as_f64())
}

/// Reads property `name` of the mpv whose IPC socket is `socket`, as mpv gives it; `null` when
/// it is unavailable.
pub fn mpv_value(socket: &Path, name: &str) -> Result<serde_json::Value, String> {
    let mut stream = UnixStream::connect(socket).map_err(|e| format!("mpv's IPC: {e}"))?;
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .map_err(|e| format!("mpv's IPC: {e}"))?;
    let request = serde_json::json!({"command": ["get_property", name], "request_id": 1});
    writeln!(stream, "{request}").map_err(|e| format!("mpv's IPC: {e}"))?;
    for line in BufReader::new(stream).lines() {
        let line = line.map_err(|e| format!("mpv's IPC: {e}"))?;
        let reply: serde_json::Value =
            serde_json::from_str(&line).map_err(|e| format!("mpv's IPC: {e}"))?;
        // Events come on the same socket; the reply carries the request's id.
        if reply["request_id"] == 1 {
            return Ok(reply["data"].clone());
        }
    }
    Err("mpv's IPC closed without a reply".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presentation_lines_are_read_and_the_unreadable_counted() {
        let log = "presentation-shm: feedback mode\n\
                   \u{20}    1: f2c  0 ms, c2p  4 ms, f2p  4 ms, p2p     0 us, t2p   4170, [s___], seq 7\n\
                   \u{20}    2: f2c  1 ms, c2p  3 ms, f2p  4 ms, p2p  4166 us, t2p  -12, [sc_z], seq 8\n\
                   \u{20}    3: f2c  1 ms, c2p  3 ms, f2p  4 ms, p2p    cut\n\
                   discarded 3\n\
                   presentation-shm exiting\n";
        assert_eq!(
            parse_presentation_log(log),
            PresentationLog {
                between_us: vec![0, 4166],
                discarded: 1,
                unread: 1,
            }
        );
    }

    #[test]
    fn kwins_rows_are_counted_by_their_presentation_time() {
        let csv = "target pageflip timestamp,pageflip timestamp,render start,render end,\
                   safety margin,refresh duration,vrr,tearing,predicted render time\n\
                   100,110,90,95,1000,4166666,0,0,500\n\
                   200,210,190,195,1000,4166666,0,0,500\n\
                   300,310,290,295,1000,4166667,0,0,500\n\
                   garbage\n";
        assert_eq!(kwin_frames(csv, 105, 250), (2, Some(4_166_666.0)));
        assert_eq!(kwin_frames(csv, 400, 500), (0, None));
    }

    #[test]
    fn yunas_output_and_the_counters_frames_are_read() {
        let log = "DEBUG yuna::frames: frame drawn output=1 took_us=200\n";
        let counter = "session 3840x2160\nframe 100\nframe 200\nframe 300\n";
        assert_eq!(counted_frames(counter, 150, 300), 2);
        let created = "INFO yuna::output: virtual output created output=1 width=3840 height=2160 \
                       millihertz=240000 scale=1.5\n";
        assert_eq!(
            yuna_output(created).as_deref(),
            Some("3840x2160@240.000,1.5")
        );
        assert_eq!(yuna_output(log), None);
    }

    #[test]
    fn percentiles_take_the_nearest_rank() {
        let values: Vec<f64> = (1..=100).map(f64::from).collect();
        assert_eq!(percentile(&values, 50.0), Some(50.0));
        assert_eq!(percentile(&values, 99.0), Some(99.0));
        assert_eq!(percentile(&[7.0], 99.0), Some(7.0));
        assert_eq!(percentile(&[], 50.0), None);
    }
}
