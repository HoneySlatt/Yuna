//! What the threads and the DRM clients of a set of processes did over a measured window.
//!
//! The kernel keeps a thread's CPU time and context switches only while the thread lives: a
//! difference of two readings at the window's ends would lose every thread that ended between
//! them. A [`Window`] reads them every [`PERIOD`] and keeps each thread's last reading, so that a
//! thread that ends inside the window counts up to its last reading, and one that starts inside it
//! counts from zero. DRM clients are kept the same way. The control group's `cpu.stat` gives the
//! exact total of the CPU time beside it (`cgroup`); this gives it by thread, and the wake-ups,
//! which nothing else keeps.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::cgroup::Cgroup;
use crate::drm::{self, DrmFile};
use crate::procfs::Proc;
use crate::sandbox::FORBIDDEN_NODES;

/// How often a window reads its processes: a thread that ends loses at most this much of its
/// counts.
pub const PERIOD: Duration = Duration::from_millis(250);

/// A thread, by its process and its own id.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct ThreadId {
    /// Its process.
    pub pid: u32,
    /// Its own id.
    pub tid: u32,
}

/// What a thread has done so far, or over a window.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ThreadCounts {
    /// Its process's name.
    pub process: String,
    /// Its own name.
    pub name: String,
    /// Its time on a core, in nanoseconds.
    pub cpu_ns: u64,
    /// The times it slept, waiting for something: its wake-ups.
    pub voluntary: u64,
    /// The times the scheduler took its core from it.
    pub involuntary: u64,
}

impl ThreadCounts {
    fn since(&self, before: &ThreadCounts) -> ThreadCounts {
        ThreadCounts {
            process: self.process.clone(),
            name: self.name.clone(),
            cpu_ns: self.cpu_ns.saturating_sub(before.cpu_ns),
            voluntary: self.voluntary.saturating_sub(before.voluntary),
            involuntary: self.involuntary.saturating_sub(before.involuntary),
        }
    }

    /// Whether a later reading of the same id went back: the id is another thread's now.
    fn went_back(&self, later: &ThreadCounts) -> bool {
        later.cpu_ns < self.cpu_ns
            || later.voluntary < self.voluntary
            || later.involuntary < self.involuntary
    }
}

/// One reading of a set of processes.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Sample {
    /// Each live thread.
    pub threads: BTreeMap<ThreadId, ThreadCounts>,
    /// Each DRM client's engines' busy times in nanoseconds, by [`DrmFile::client_key`].
    pub drm: BTreeMap<String, BTreeMap<String, u64>>,
    /// The forbidden nodes the processes held open, each with its process: a card node, which
    /// drives the owner's display, or an input device ([`FORBIDDEN_NODES`]). A case that holds
    /// one is refused.
    pub forbidden: Vec<(u32, String)>,
}

impl Sample {
    /// Reads processes `pids` now.
    pub fn take(proc: &Proc, pids: &[u32]) -> Sample {
        let mut sample = Sample::default();
        for &pid in pids {
            let process = proc.name(pid).unwrap_or_default();
            for thread in proc.threads(pid) {
                sample.threads.insert(
                    ThreadId {
                        pid,
                        tid: thread.tid,
                    },
                    ThreadCounts {
                        process: process.clone(),
                        name: thread.name,
                        cpu_ns: thread.cpu_ns,
                        voluntary: thread.voluntary,
                        involuntary: thread.involuntary,
                    },
                );
            }
            for (_, target) in proc.open_files(pid) {
                let node = target.to_string_lossy();
                if FORBIDDEN_NODES
                    .iter()
                    .any(|forbidden| node.starts_with(forbidden))
                {
                    sample.forbidden.push((pid, node.into_owned()));
                }
            }
        }
        let files = drm::drm_files(proc, pids);
        for (key, file) in drm::distinct_clients(&files) {
            sample.drm.insert(key, file.info.engines.clone());
        }
        sample
    }
}

/// What a set of processes did over a window, from its readings.
#[derive(Debug, Clone, Default)]
pub struct Window {
    baseline: BTreeMap<ThreadId, ThreadCounts>,
    last: BTreeMap<ThreadId, ThreadCounts>,
    /// What threads whose id was taken again within the window had done in it.
    replaced: Vec<ThreadCounts>,
    live: BTreeSet<ThreadId>,
    drm_baseline: BTreeMap<String, BTreeMap<String, u64>>,
    drm_last: BTreeMap<String, BTreeMap<String, u64>>,
    forbidden: BTreeSet<(u32, String)>,
    readings: usize,
}

/// One thread's counts over a window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadInWindow {
    /// The thread.
    pub id: ThreadId,
    /// What it did in the window.
    pub counts: ThreadCounts,
    /// Whether it had ended by the window's last reading: then it counts up to its own last.
    pub ended: bool,
}

impl Window {
    /// A window that starts at `first`.
    pub fn start(first: Sample) -> Window {
        let mut window = Window {
            baseline: first.threads.clone(),
            drm_baseline: first.drm.clone(),
            ..Window::default()
        };
        window.observe(first);
        window
    }

    /// Adds a later reading.
    pub fn observe(&mut self, sample: Sample) {
        self.readings += 1;
        self.live = sample.threads.keys().copied().collect();
        for (id, counts) in sample.threads {
            if let Some(last) = self.last.get(&id)
                && last.went_back(&counts)
            {
                // The id was freed and taken again: keep what the first thread did, and count
                // the new one from zero.
                let before = self.baseline.remove(&id).unwrap_or_default();
                self.replaced.push(last.since(&before));
            }
            self.last.insert(id, counts);
        }
        for (client, engines) in sample.drm {
            self.drm_last.insert(client, engines);
        }
        self.forbidden.extend(sample.forbidden);
    }

    /// How many readings it holds.
    pub fn readings(&self) -> usize {
        self.readings
    }

    /// Each thread seen, with what it did in the window.
    pub fn threads(&self) -> Vec<ThreadInWindow> {
        self.last
            .iter()
            .map(|(id, last)| ThreadInWindow {
                id: *id,
                counts: last.since(self.baseline.get(id).unwrap_or(&ThreadCounts::default())),
                ended: !self.live.contains(id),
            })
            .collect()
    }

    /// The counts of every thread together: those seen, those whose id was taken again.
    pub fn total(&self) -> ThreadCounts {
        let mut total = ThreadCounts::default();
        let threads = self.threads();
        for counts in threads
            .iter()
            .map(|thread| &thread.counts)
            .chain(&self.replaced)
        {
            total.cpu_ns += counts.cpu_ns;
            total.voluntary += counts.voluntary;
            total.involuntary += counts.involuntary;
        }
        total
    }

    /// Each engine's busy time in the window, in nanoseconds, every client together: a client
    /// that started in it counts from zero, one that ended counts up to its last reading.
    pub fn engines(&self) -> BTreeMap<String, u64> {
        let mut sum = BTreeMap::new();
        for (client, last) in &self.drm_last {
            let before = self.drm_baseline.get(client);
            for (engine, ns) in last {
                let start = before.and_then(|b| b.get(engine)).copied().unwrap_or(0);
                *sum.entry(engine.clone()).or_default() += ns.saturating_sub(start);
            }
        }
        sum
    }

    /// How many DRM clients it saw.
    pub fn drm_clients(&self) -> usize {
        self.drm_last.len()
    }

    /// The forbidden nodes any reading found open, each with its process.
    pub fn forbidden(&self) -> Vec<(u32, String)> {
        self.forbidden.iter().cloned().collect()
    }
}

/// Windows over several control groups, read on a thread of its own every [`PERIOD`].
pub struct Watch {
    windows: Arc<Mutex<Vec<(String, Window)>>>,
    stop: Arc<AtomicBool>,
    thread: std::thread::JoinHandle<()>,
}

impl Watch {
    /// Starts a window over each of `groups`, by their names.
    pub fn start(proc: &Proc, groups: Vec<(String, Cgroup)>) -> Watch {
        let windows: Vec<(String, Window)> = groups
            .iter()
            .map(|(name, group)| {
                (
                    name.clone(),
                    Window::start(Sample::take(proc, &group.processes())),
                )
            })
            .collect();
        let windows = Arc::new(Mutex::new(windows));
        let stop = Arc::new(AtomicBool::new(false));
        let thread = {
            let (windows, stop, proc) = (Arc::clone(&windows), Arc::clone(&stop), proc.clone());
            std::thread::spawn(move || {
                let mut next = Instant::now() + PERIOD;
                while !stop.load(Ordering::Relaxed) {
                    // Woken early by `finish`, which takes the last reading itself.
                    std::thread::park_timeout(next.saturating_duration_since(Instant::now()));
                    if stop.load(Ordering::Relaxed) {
                        break;
                    }
                    if Instant::now() < next {
                        continue;
                    }
                    next += PERIOD;
                    let samples: Vec<Sample> = groups
                        .iter()
                        .map(|(_, group)| Sample::take(&proc, &group.processes()))
                        .collect();
                    let mut windows = windows.lock().unwrap_or_else(|e| e.into_inner());
                    for ((_, window), sample) in windows.iter_mut().zip(samples) {
                        window.observe(sample);
                    }
                }
            })
        };
        Watch {
            windows,
            stop,
            thread,
        }
    }

    /// The forbidden nodes found open so far, in any group, each with its process.
    pub fn forbidden(&self) -> Vec<(u32, String)> {
        let windows = self.windows.lock().unwrap_or_else(|e| e.into_inner());
        windows
            .iter()
            .flat_map(|(_, window)| window.forbidden())
            .collect()
    }

    /// Takes a last reading of each group at once, and gives the windows, by their names.
    pub fn finish(self, proc: &Proc, groups: &[(String, Cgroup)]) -> Vec<(String, Window)> {
        self.stop.store(true, Ordering::Relaxed);
        self.thread.thread().unpark();
        let _ = self.thread.join();
        let mut windows =
            std::mem::take(&mut *self.windows.lock().unwrap_or_else(|e| e.into_inner()));
        for ((_, window), (_, group)) in windows.iter_mut().zip(groups) {
            window.observe(Sample::take(proc, &group.processes()));
        }
        windows
    }
}

/// The DRM files of a group's processes now, their `fdinfo` kept as the kernel wrote it.
pub fn drm_now(proc: &Proc, group: &Cgroup) -> Vec<DrmFile> {
    drm::drm_files(proc, &group.processes())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    fn thread(cpu_ns: u64, voluntary: u64, involuntary: u64) -> ThreadCounts {
        ThreadCounts {
            process: "sway".to_owned(),
            name: "sway".to_owned(),
            cpu_ns,
            voluntary,
            involuntary,
        }
    }

    fn sample(threads: &[(u32, ThreadCounts)]) -> Sample {
        Sample {
            threads: threads
                .iter()
                .map(|(tid, counts)| (ThreadId { pid: 1, tid: *tid }, counts.clone()))
                .collect(),
            ..Sample::default()
        }
    }

    #[test]
    fn a_thread_that_ends_mid_window_counts_up_to_its_last_reading() {
        // Thread 2 wakes 5 times, then ends before the window's end; thread 3 starts within the
        // window, wakes 7 times and ends too; thread 1 lives throughout.
        let mut window = Window::start(sample(&[(1, thread(100, 10, 1)), (2, thread(50, 20, 0))]));
        window.observe(sample(&[
            (1, thread(130, 12, 1)),
            (2, thread(80, 25, 2)),
            (3, thread(40, 7, 1)),
        ]));
        window.observe(sample(&[(1, thread(160, 16, 2))]));
        let total = window.total();
        assert_eq!(total.voluntary, 6 + 5 + 7);
        assert_eq!(total.involuntary, 1 + 2 + 1);
        assert_eq!(total.cpu_ns, 60 + 30 + 40);
        let ended: Vec<u32> = window
            .threads()
            .iter()
            .filter(|thread| thread.ended)
            .map(|thread| thread.id.tid)
            .collect();
        assert_eq!(ended, [2, 3]);
        assert_eq!(window.readings(), 3);
    }

    #[test]
    fn a_thread_id_taken_again_keeps_both_threads_counts() {
        let mut window = Window::start(sample(&[(2, thread(50, 20, 0))]));
        window.observe(sample(&[(2, thread(90, 30, 0))]));
        // Thread 2 ended; a new thread took its id, and has woken 3 times since it started.
        window.observe(sample(&[(2, thread(10, 3, 0))]));
        let total = window.total();
        assert_eq!(total.voluntary, 10 + 3);
        assert_eq!(total.cpu_ns, 40 + 10);
    }

    #[test]
    fn a_reading_finds_every_forbidden_node_the_checks_refuse() {
        let root = crate::testing::folder("sampler-forbidden");
        fs::create_dir_all(root.join("10/fd")).unwrap();
        fs::write(root.join("10/comm"), "compositor\n").unwrap();
        for (fd, node) in [
            ("3", "/dev/dri/renderD128"),
            ("4", "/dev/dri/card1"),
            ("5", "/dev/input/event3"),
            ("6", "/dev/uinput"),
            ("7", "/dev/hidraw0"),
        ] {
            std::os::unix::fs::symlink(node, root.join("10/fd").join(fd)).unwrap();
        }
        let sample = Sample::take(&Proc::at(&root), &[10]);
        let nodes: Vec<&str> = sample
            .forbidden
            .iter()
            .map(|(_, node)| node.as_str())
            .collect();
        assert_eq!(
            nodes,
            [
                "/dev/dri/card1",
                "/dev/input/event3",
                "/dev/uinput",
                "/dev/hidraw0"
            ]
        );
        assert!(sample.forbidden.iter().all(|(pid, _)| *pid == 10));
        let window = Window::start(sample);
        assert_eq!(window.forbidden().len(), 4);
    }

    #[test]
    fn drm_clients_that_start_or_end_in_the_window_count() {
        let engines = |gfx: u64| BTreeMap::from([("gfx".to_owned(), gfx)]);
        let mut first = Sample::default();
        first.drm.insert("a".to_owned(), engines(1_000));
        first.drm.insert("b".to_owned(), engines(500));
        let mut window = Window::start(first);
        let mut second = Sample::default();
        second.drm.insert("a".to_owned(), engines(1_600));
        second.drm.insert("b".to_owned(), engines(800));
        second.drm.insert("c".to_owned(), engines(200));
        window.observe(second);
        // b ended; a and c carry on.
        let mut third = Sample::default();
        third.drm.insert("a".to_owned(), engines(2_000));
        third.drm.insert("c".to_owned(), engines(250));
        window.observe(third);
        assert_eq!(window.engines()["gfx"], 1_000 + 300 + 250);
        assert_eq!(window.drm_clients(), 3);
    }
}
