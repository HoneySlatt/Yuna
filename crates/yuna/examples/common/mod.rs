//! What the measures share: the run's folder and the fields of Yuna's log lines. Beside it, each
//! included by the measures that need it: the CPU and GPU time Yuna and its clients used, in
//! `usage.rs`; the machine's other programs' load, in `load.rs`; the `vkcube` clients that
//! `frame_time` and `motion_time` start, in `vkcube.rs`.

use std::fs::{self, DirBuilder};
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// The longest Yuna and its clients may take to start and show their windows.
pub const PATIENCE: Duration = Duration::from_secs(60);

/// The run's folder: a runtime folder, Yuna's log, and the clients' homes and logs. Removed at the
/// end, unless [`Run::keep`] kept it.
pub struct Run {
    folder: PathBuf,
}

impl Run {
    /// A folder `<name>-<pid>` in the temporary folder, with its `runtime` folder.
    pub fn new(name: &str) -> Result<Run, String> {
        let folder = std::env::temp_dir().join(format!("{name}-{}", std::process::id()));
        // Wayland asks that the runtime folder be the user's alone.
        DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(folder.join("runtime"))
            .map_err(|e| format!("cannot create {}: {e}", folder.display()))?;
        Ok(Run { folder })
    }

    pub fn folder(&self) -> &Path {
        &self.folder
    }

    /// Keeps the folder, for its logs, and names it.
    pub fn keep(&self) -> String {
        let kept = self.folder.with_extension("kept");
        let _ = fs::rename(&self.folder, &kept);
        kept.display().to_string()
    }
}

impl Drop for Run {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.folder);
    }
}

/// A variable the development shell sets.
pub fn shell_variable(name: &str) -> Result<String, String> {
    std::env::var(name).map_err(|_| format!("{name} is not set: run in the shell, `nix develop`"))
}

/// The text of `key=<text>` in a log line: the word after `key=`, up to the next space.
pub fn text_field<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    line.split_whitespace()
        .find_map(|word| word.strip_prefix(key)?.strip_prefix('='))
}

/// The value of `key=<number>` in a log line.
pub fn field(line: &str, key: &str) -> Option<u64> {
    text_field(line, key).and_then(|value| value.parse().ok())
}

/// The value at percentile `p` of `sorted`, by the nearest rank. `sorted` must not be empty.
pub fn percentile(sorted: &[u64], p: f64) -> u64 {
    let rank = ((p / 100.0) * sorted.len() as f64).ceil() as usize;
    sorted[rank.clamp(1, sorted.len()) - 1]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percentiles_take_the_nearest_rank() {
        let values: Vec<u64> = (1..=100).collect();
        assert_eq!(percentile(&values, 50.0), 50);
        assert_eq!(percentile(&values, 99.0), 99);
        assert_eq!(percentile(&[7], 99.0), 7);
    }

    #[test]
    fn fields_are_read() {
        let line = "DEBUG yuna::frames: frame drawn output=1 elements=2 took_us=4170";
        assert_eq!(field(line, "took_us"), Some(4170));
        assert_eq!(field(line, "took"), None);
    }

    #[test]
    fn text_fields_are_read_up_to_the_next_space() {
        let line = "INFO motion_time: operation started operation=focus-right windows=";
        assert_eq!(text_field(line, "operation"), Some("focus-right"));
        assert_eq!(text_field(line, "windows"), Some(""));
        assert_eq!(text_field(line, "phase"), None);
    }
}
