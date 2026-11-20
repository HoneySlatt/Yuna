//! The CPU time of a process's threads, from `/proc/<pid>/task/<tid>/schedstat` (nanoseconds on
//! a core) and `comm` (the thread's name), read before and after a measured window. Shared with
//! `prototypes/llvmpipe-scene`, which reads Yuna's threads as `scene` reads the prototype's own.

use std::collections::BTreeMap;
use std::fs;

use crate::json::Json;

/// Each thread of a process at one moment: its id, its name and its CPU time in nanoseconds.
pub struct Threads {
    pid: u32,
    times: BTreeMap<u32, (String, u64)>,
}

impl Threads {
    /// Reads every thread of process `pid`; a thread that ends while it is read is left out.
    pub fn read(pid: u32) -> Self {
        let mut times = BTreeMap::new();
        let folder = format!("/proc/{pid}/task");
        for entry in fs::read_dir(&folder).into_iter().flatten().flatten() {
            let Some(tid) = entry
                .file_name()
                .to_str()
                .and_then(|name| name.parse().ok())
            else {
                continue;
            };
            let read = |file: &str| fs::read_to_string(format!("{folder}/{tid}/{file}")).ok();
            let (Some(name), Some(schedstat)) = (read("comm"), read("schedstat")) else {
                continue;
            };
            let Some(ns) = schedstat
                .split_whitespace()
                .next()
                .and_then(|field| field.parse().ok())
            else {
                continue;
            };
            times.insert(tid, (name.trim().to_owned(), ns));
        }
        Self { pid, times }
    }
}

/// The CPU time a process's threads used between two readings, by kind of thread: its main
/// thread, then each other name without its number, so that `llvmpipe-0` to `llvmpipe-15` count
/// as one kind.
pub struct Usage {
    /// Each kind: its name, how many threads, their nanoseconds.
    pub kinds: Vec<(String, usize, u64)>,
    pub total_ns: u64,
}

impl Usage {
    pub fn between(before: &Threads, after: &Threads) -> Self {
        let mut kinds: BTreeMap<String, (usize, u64)> = BTreeMap::new();
        let mut total_ns = 0;
        for (tid, (name, after_ns)) in &after.times {
            // A thread started within the window counts from its start.
            let before_ns = before.times.get(tid).map_or(0, |(_, ns)| *ns);
            let used = after_ns.saturating_sub(before_ns);
            let kind = if *tid == after.pid {
                "main thread".to_owned()
            } else {
                thread_kind(name).to_owned()
            };
            let entry = kinds.entry(kind).or_default();
            entry.0 += 1;
            entry.1 += used;
            total_ns += used;
        }
        Self {
            kinds: kinds
                .into_iter()
                .map(|(kind, (threads, ns))| (kind, threads, ns))
                .collect(),
            total_ns,
        }
    }

    /// Microseconds a frame, over `frames` frames: the total, then each kind that ran.
    pub fn to_json(&self, frames: usize) -> Json {
        let per_frame = |ns: u64| Json::Number(ns as f64 / 1000.0 / frames.max(1) as f64);
        Json::Object(vec![
            ("total_us_a_frame", per_frame(self.total_ns)),
            (
                "kinds",
                Json::Array(
                    self.kinds
                        .iter()
                        .filter(|(_, _, ns)| *ns > 0)
                        .map(|(kind, threads, ns)| {
                            Json::Object(vec![
                                ("kind", Json::text(kind)),
                                ("threads", Json::Integer(*threads as i64)),
                                ("us_a_frame", per_frame(*ns)),
                            ])
                        })
                        .collect(),
                ),
            ),
        ])
    }

    /// "total µs: kind (threads) µs, ...", a frame, over `frames` frames.
    pub fn to_text(&self, frames: usize) -> String {
        let per_frame = |ns: u64| ns as f64 / 1000.0 / frames.max(1) as f64;
        let kinds: Vec<String> = self
            .kinds
            .iter()
            .filter(|(_, _, ns)| *ns > 0)
            .map(|(kind, threads, ns)| format!("{kind} ({threads}) {:.0}", per_frame(*ns)))
            .collect();
        format!("{:.0} µs: {}", per_frame(self.total_ns), kinds.join(", "))
    }
}

/// A thread's name without its number: `llvmpipe-12` is `llvmpipe`.
fn thread_kind(name: &str) -> &str {
    name.trim_end_matches(|c: char| c.is_ascii_digit())
        .trim_end_matches(['-', ':', '_'])
}

#[cfg(test)]
mod tests {
    use super::{Threads, Usage, thread_kind};
    use std::collections::BTreeMap;

    #[test]
    fn counts_each_kind_of_thread() {
        assert_eq!(thread_kind("llvmpipe-12"), "llvmpipe");
        assert_eq!(thread_kind("yuna"), "yuna");
        let threads = |entries: &[(u32, &str, u64)]| Threads {
            pid: 10,
            times: entries
                .iter()
                .map(|&(tid, name, ns)| (tid, (name.to_owned(), ns)))
                .collect::<BTreeMap<_, _>>(),
        };
        let before = threads(&[(10, "main", 1000), (11, "llvmpipe-0", 500)]);
        let after = threads(&[
            (10, "main", 3000),
            (11, "llvmpipe-0", 1500),
            (12, "llvmpipe-1", 700),
        ]);
        let usage = Usage::between(&before, &after);
        assert_eq!(usage.total_ns, 3700);
        assert_eq!(
            usage.kinds,
            vec![
                ("llvmpipe".to_owned(), 2, 1700),
                ("main thread".to_owned(), 1, 2000)
            ]
        );
        assert_eq!(usage.to_text(1), "4 µs: llvmpipe (2) 2, main thread (1) 2");
        let reading = Threads::read(std::process::id());
        assert!(reading.times.contains_key(&std::process::id()));
    }
}
