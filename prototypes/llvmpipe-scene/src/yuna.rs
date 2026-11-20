//! `yuna --headless`, started as Yuna's `frame_time` example starts it: on llvmpipe, its default
//! output, logging at `debug` to a file, with nothing of this program's environment but what it
//! draws with, and Mesa's `LP_NUM_THREADS` when set. Its log's `frame drawn` lines give each
//! frame's time, `took_us`: from when Yuna starts to build the frame, its clients' buffers
//! uploaded, to when the drawing is done.

use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

/// How long Yuna has to print its socket.
const PATIENCE: Duration = Duration::from_secs(20);

/// The variables of this program's environment that Yuna draws with: the Nix shell's EGL
/// dispatcher and Mesa, and its keyboard's data.
const KEPT_VARIABLES: [&str; 3] = [
    "LD_LIBRARY_PATH",
    "__EGL_VENDOR_LIBRARY_FILENAMES",
    "XKB_CONFIG_ROOT",
];

/// A running Yuna, killed when dropped.
pub struct Yuna {
    child: Child,
    pub log: PathBuf,
    /// The path of its Wayland socket.
    pub socket: PathBuf,
}

impl Yuna {
    /// Starts `program` with its runtime folder and home in `folder`, its log in `log`, at
    /// `level`.
    pub fn start(program: &Path, folder: &Path, log: &Path, level: &str) -> Result<Self, String> {
        let runtime = folder.join("runtime");
        let home = folder.join("home");
        for path in [&runtime, &home] {
            fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(path)
                .map_err(|error| format!("cannot make {}: {error}", path.display()))?;
        }
        let mut command = Command::new(program);
        command.arg("--headless").env_clear();
        for name in KEPT_VARIABLES {
            let value = std::env::var(name)
                .map_err(|_| format!("{name} is not set: run inside `nix develop`"))?;
            command.env(name, value);
        }
        if let Ok(threads) = std::env::var("LP_NUM_THREADS") {
            command.env("LP_NUM_THREADS", threads);
        }
        command
            .env("HOME", &home)
            .env("XDG_RUNTIME_DIR", &runtime)
            .env("YUNA_LOG", level)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(
                File::create(log)
                    .map_err(|error| format!("cannot make {}: {error}", log.display()))?,
            );
        let mut child = command
            .spawn()
            .map_err(|error| format!("{} does not start: {error}", program.display()))?;
        let stdout = child.stdout.take().ok_or("Yuna's standard output")?;
        // Read on a thread of its own, so that a Yuna that never prints fails in time.
        let (sender, receiver) = mpsc::channel();
        std::thread::spawn(move || {
            let mut line = String::new();
            let _ = BufReader::new(stdout).read_line(&mut line);
            let _ = sender.send(line);
        });
        let line = receiver.recv_timeout(PATIENCE).unwrap_or_default();
        let mut yuna = Self {
            child,
            log: log.to_owned(),
            socket: PathBuf::new(),
        };
        let name = line
            .trim_end()
            .strip_prefix("WAYLAND_DISPLAY=")
            .ok_or_else(|| format!("yuna printed no socket: {line:?}"))?;
        yuna.socket = runtime.join(name);
        Ok(yuna)
    }

    pub fn pid(&self) -> u32 {
        self.child.id()
    }

    /// The log's length up to its last whole line: a mark of the measured window.
    pub fn log_length(&self) -> Result<usize, String> {
        let log = self.read_log()?;
        Ok(log.rfind('\n').map_or(0, |end| end + 1))
    }

    pub fn read_log(&self) -> Result<String, String> {
        fs::read_to_string(&self.log).map_err(|error| format!("Yuna's log: {error}"))
    }

    /// Whether Yuna still runs.
    pub fn running(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }
}

impl Drop for Yuna {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// What the `frame drawn` lines of a part of Yuna's log say.
pub struct Frames {
    /// Each frame's `took_us`.
    pub took_us: Vec<f64>,
    /// The boundary each frame aimed at, in the output's periods.
    pub boundaries: Vec<u64>,
    /// The fewest and most rectangles a frame damaged, and elements it drew.
    pub damaged_rectangles: (u64, u64),
    pub elements: (u64, u64),
}

pub fn frames(log: &str) -> Frames {
    let mut frames = Frames {
        took_us: Vec::new(),
        boundaries: Vec::new(),
        damaged_rectangles: (u64::MAX, 0),
        elements: (u64::MAX, 0),
    };
    for line in log.lines().filter(|line| line.contains("frame drawn")) {
        if let Some(took) = field(line, "took_us") {
            frames.took_us.push(took as f64);
        }
        frames.boundaries.extend(field(line, "boundary"));
        for (key, range) in [
            ("damaged_rectangles", &mut frames.damaged_rectangles),
            ("elements", &mut frames.elements),
        ] {
            if let Some(value) = field(line, key) {
                *range = (range.0.min(value), range.1.max(value));
            }
        }
    }
    frames
}

/// The value of `key=<number>` in a log line, as `frame_time` reads it.
fn field(line: &str, key: &str) -> Option<u64> {
    line.split_whitespace()
        .find_map(|word| word.strip_prefix(key)?.strip_prefix('='))
        .and_then(|value| value.parse().ok())
}

#[cfg(test)]
mod tests {
    use super::frames;

    #[test]
    fn reads_the_frames_lines() {
        let log = "\
            DEBUG yuna::frames: frame drawn output=1 elements=2 damaged_rectangles=2 took_us=6900 boundary=7\n\
            DEBUG yuna: counters frames_drawn=1\n\
            DEBUG yuna::frames: frame drawn output=1 elements=2 damaged_rectangles=1 took_us=310 boundary=10\n";
        let read = frames(log);
        assert_eq!(read.took_us, vec![6900.0, 310.0]);
        assert_eq!(read.boundaries, vec![7, 10]);
        assert_eq!(read.damaged_rectangles, (1, 2));
        assert_eq!(read.elements, (2, 2));
    }
}
