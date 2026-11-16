//! What the measures share: the run's folder, the fields of Yuna's log lines, and the CPU and GPU
//! time read from `/proc`, by the readers of `yuna_bench`
//! ([ADR-0030](../../../../docs/adr/0030-a-frames-gpu-time-from-the-kernel.md)).
//! The `vkcube` clients that `frame_time` and `motion_time` start are in `vkcube.rs` beside it.

use std::collections::BTreeMap;
use std::fs::{self, DirBuilder};
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::process::Child;
use std::time::{Duration, Instant};

use yuna_bench::drm;
use yuna_bench::procfs::Proc;

/// How long the clients draw before anything is measured.
pub const WARM_UP: Duration = Duration::from_secs(3);
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

/// What Yuna and its clients have used, read at one end of a measured window.
pub struct Usage {
    pub at: Instant,
    /// Yuna's process id, which is also its main thread's.
    pub pid: u32,
    /// Each of Yuna's threads: its name, and its CPU time in nanoseconds.
    pub yuna_threads: BTreeMap<u32, (String, u64)>,
    /// The GPU's busy time for Yuna, per engine, in nanoseconds.
    pub yuna_gpu: BTreeMap<String, u64>,
    /// Each of the clients' threads, and its CPU time in nanoseconds.
    pub clients_threads: BTreeMap<u32, u64>,
    /// The GPU's busy time for all the clients, per engine, in nanoseconds.
    pub clients_gpu: BTreeMap<String, u64>,
    /// The files Yuna holds open that are dma-bufs, and memfds.
    pub yuna_dmabufs: usize,
    pub yuna_memfds: usize,
}

impl Usage {
    /// Reads what process `pid`, Yuna, and `clients` have used so far.
    pub fn take(pid: u32, clients: &[Child]) -> Result<Usage, String> {
        let proc = Proc::system();
        let mut clients_threads = BTreeMap::new();
        for client in clients {
            let client_threads = threads(&proc, client.id())?;
            clients_threads.extend(client_threads.into_iter().map(|(tid, (_, ns))| (tid, ns)));
        }
        let client_pids: Vec<u32> = clients.iter().map(Child::id).collect();
        let (yuna_dmabufs, yuna_memfds) = buffer_files(&proc, pid);
        Ok(Usage {
            at: Instant::now(),
            pid,
            yuna_threads: threads(&proc, pid)?,
            yuna_gpu: gpu_engines(&proc, &[pid]),
            clients_threads,
            clients_gpu: gpu_engines(&proc, &client_pids),
            yuna_dmabufs,
            yuna_memfds,
        })
    }
}

/// Each thread of process `pid`: its name, and the CPU time it has used, in nanoseconds. A
/// process that has ended has none: that is an error, as a measure of it would be empty.
fn threads(proc: &Proc, pid: u32) -> Result<BTreeMap<u32, (String, u64)>, String> {
    let threads = proc.threads(pid);
    if threads.is_empty() {
        return Err(format!("process {pid} has no thread to read: it has ended"));
    }
    Ok(threads
        .into_iter()
        .map(|thread| (thread.tid, (thread.name, thread.cpu_ns)))
        .collect())
}

/// The GPU's busy time for the processes `pids`, per engine, in nanoseconds, each DRM client
/// counted once however many files or processes share it.
fn gpu_engines(proc: &Proc, pids: &[u32]) -> BTreeMap<String, u64> {
    drm::engines(&drm::drm_files(proc, pids))
}

/// How many of process `pid`'s open files are dma-bufs, and how many memfds: the buffers its
/// clients share with it.
fn buffer_files(proc: &Proc, pid: u32) -> (usize, usize) {
    let (mut dmabufs, mut memfds) = (0, 0);
    for (_, target) in proc.open_files(pid) {
        let target = target.to_string_lossy();
        if target.starts_with("/dmabuf:") {
            dmabufs += 1;
        } else if target.starts_with("/memfd:") {
            memfds += 1;
        }
    }
    (dmabufs, memfds)
}

/// A thread's CPU time since `before_ns`, its time at the window's start, or since it started if
/// it started within the window. Each thread's time is taken apart, so that a thread that ended
/// within the window never makes a difference of totals go below zero.
fn used_since(after_ns: u64, before_ns: Option<u64>) -> u64 {
    after_ns.saturating_sub(before_ns.unwrap_or(0))
}

/// A thread's name without its number, so that `llvmpipe-0` to `llvmpipe-15` count as one.
fn thread_kind(name: &str) -> &str {
    name.trim_end_matches(|c: char| c.is_ascii_digit())
        .trim_end_matches(['-', ':', '_'])
}

/// Prints what Yuna and its clients used between `before` and `after`, per frame of the `frames`
/// Yuna drew between them: Yuna's CPU time, llvmpipe's rasterizer apart, by kind of thread; the
/// GPU's time for Yuna and for its clients; the clients' CPU time; and the buffers Yuna holds at
/// the end. `frames` must not be 0.
pub fn print_usage(before: &Usage, after: &Usage, frames: u64) {
    let seconds = (after.at - before.at).as_secs_f64();
    let per_frame = |ns: u64| ns as f64 / frames as f64 / 1_000.0;
    let mut kinds: BTreeMap<&str, (usize, u64)> = BTreeMap::new();
    let (mut cpu, mut rasterizer) = (0, 0);
    for (tid, (name, after_ns)) in &after.yuna_threads {
        let before_ns = before.yuna_threads.get(tid).map(|(_, ns)| *ns);
        let used = used_since(*after_ns, before_ns);
        // The main thread runs the event loop and Yuna's own work; the others are Mesa's.
        let kind = if *tid == after.pid {
            "main thread"
        } else {
            thread_kind(name)
        };
        // llvmpipe's threads rasterize: the work a GPU would do (ADR-0030).
        if kind == "llvmpipe" {
            rasterizer += used;
        } else {
            cpu += used;
        }
        let kind = kinds.entry(kind).or_default();
        kind.0 += 1;
        kind.1 += used;
    }
    println!(
        "Yuna's CPU time per frame, llvmpipe's rasterizer apart: {:.0} µs ({:.2} cores busy)",
        per_frame(cpu),
        cpu as f64 / 1e9 / seconds,
    );
    if rasterizer > 0 {
        println!(
            "llvmpipe's rasterizer per frame, in place of a GPU: {:.0} µs ({:.2} cores busy)",
            per_frame(rasterizer),
            rasterizer as f64 / 1e9 / seconds,
        );
    }
    println!("  by thread, of Yuna's {}:", after.yuna_threads.len());
    for (kind, (count, used)) in &kinds {
        if *used > 0 {
            println!("  {kind} ({count} threads): {:.0} µs", per_frame(*used));
        }
    }
    let gpu = |after: &BTreeMap<String, u64>, before: &BTreeMap<String, u64>| {
        let engines = after
            .iter()
            .map(|(engine, ns)| {
                let used = ns.saturating_sub(before.get(engine).copied().unwrap_or(0));
                format!("{engine} {:.0} µs", per_frame(used))
            })
            .collect::<Vec<_>>();
        if engines.is_empty() {
            "no DRM file open".to_owned()
        } else {
            engines.join(", ")
        }
    };
    println!(
        "Yuna's GPU time per frame (fdinfo): {}",
        gpu(&after.yuna_gpu, &before.yuna_gpu)
    );
    println!(
        "the clients, per frame Yuna drew: CPU {:.0} µs; GPU (fdinfo) {}",
        per_frame(
            after
                .clients_threads
                .iter()
                .map(|(tid, ns)| used_since(*ns, before.clients_threads.get(tid).copied()))
                .sum()
        ),
        gpu(&after.clients_gpu, &before.clients_gpu),
    );
    println!(
        "buffers Yuna holds at the end: {} dma-bufs, {} memfds",
        after.yuna_dmabufs, after.yuna_memfds
    );
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
    fn fields_and_thread_kinds_are_read() {
        let line = "DEBUG yuna::frames: frame drawn output=1 elements=2 took_us=4170";
        assert_eq!(field(line, "took_us"), Some(4170));
        assert_eq!(field(line, "took"), None);
        assert_eq!(thread_kind("llvmpipe-12"), "llvmpipe");
        assert_eq!(thread_kind("yuna"), "yuna");
    }

    #[test]
    fn text_fields_are_read_up_to_the_next_space() {
        let line = "INFO motion_time: operation started operation=focus-right windows=";
        assert_eq!(text_field(line, "operation"), Some("focus-right"));
        assert_eq!(text_field(line, "windows"), Some(""));
        assert_eq!(text_field(line, "phase"), None);
    }
}
