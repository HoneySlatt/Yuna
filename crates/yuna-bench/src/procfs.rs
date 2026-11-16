//! What the kernel tells of a process in `/proc`, read from outside it: its threads' CPU time and
//! context switches, its open files, its memory, its environment and its control group.
//!
//! Every reader takes the folder it reads as `/proc` ([`Proc::at`]), so that the tests give it a
//! fake one. A process or a thread that ends while it is read is left out, never an error: the
//! callers sample often enough to keep what it did before (`sampler`).

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

/// The `/proc` folder, or a folder laid out as it is.
#[derive(Debug, Clone)]
pub struct Proc {
    root: PathBuf,
}

/// One thread, as its `task/<tid>` folder gives it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadSample {
    /// Its thread id.
    pub tid: u32,
    /// Its name (`comm`).
    pub name: String,
    /// The time it ran on a core, in nanoseconds: the first field of `schedstat`.
    pub cpu_ns: u64,
    /// Its voluntary context switches: each time it slept, waiting for something.
    pub voluntary: u64,
    /// Its involuntary context switches: each time the scheduler took its core from it.
    pub involuntary: u64,
}

impl Proc {
    /// The machine's own `/proc`.
    pub fn system() -> Proc {
        Proc::at("/proc")
    }

    /// A folder laid out as `/proc`.
    pub fn at(root: impl Into<PathBuf>) -> Proc {
        Proc { root: root.into() }
    }

    fn path(&self, pid: u32, file: &str) -> PathBuf {
        self.root.join(pid.to_string()).join(file)
    }

    /// Every process id the folder lists.
    pub fn pids(&self) -> Vec<u32> {
        let mut pids: Vec<u32> = fs::read_dir(&self.root)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|entry| entry.file_name().to_str()?.parse().ok())
            .collect();
        pids.sort_unstable();
        pids
    }

    /// Each thread of process `pid`; empty once it has ended.
    pub fn threads(&self, pid: u32) -> Vec<ThreadSample> {
        let tasks = self.path(pid, "task");
        let mut threads: Vec<ThreadSample> = fs::read_dir(&tasks)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|task| {
                let tid = task.file_name().to_str()?.parse().ok()?;
                let folder = task.path();
                let name = fs::read_to_string(folder.join("comm")).ok()?;
                let cpu_ns = parse_schedstat(&fs::read_to_string(folder.join("schedstat")).ok()?)?;
                let (voluntary, involuntary) =
                    parse_context_switches(&fs::read_to_string(folder.join("status")).ok()?)?;
                Some(ThreadSample {
                    tid,
                    name: name.trim_end().to_owned(),
                    cpu_ns,
                    voluntary,
                    involuntary,
                })
            })
            .collect();
        threads.sort_by_key(|thread| thread.tid);
        threads
    }

    /// Process `pid`'s name.
    pub fn name(&self, pid: u32) -> Option<String> {
        fs::read_to_string(self.path(pid, "comm"))
            .ok()
            .map(|name| name.trim_end().to_owned())
    }

    /// Process `pid`'s parent and its processor time, its reaped children's included, in clock
    /// ticks: from its `stat`.
    pub fn stat(&self, pid: u32) -> Option<(u32, u64)> {
        parse_stat(&fs::read_to_string(self.path(pid, "stat")).ok()?)
    }

    /// What each of process `pid`'s open files is: its descriptor and the target of its link in
    /// `fd/`, such as `/dev/dri/renderD128` or `socket:[1234]`.
    pub fn open_files(&self, pid: u32) -> Vec<(u32, PathBuf)> {
        let mut files: Vec<(u32, PathBuf)> = fs::read_dir(self.path(pid, "fd"))
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|file| {
                let fd = file.file_name().to_str()?.parse().ok()?;
                Some((fd, fs::read_link(file.path()).ok()?))
            })
            .collect();
        files.sort_by_key(|(fd, _)| *fd);
        files
    }

    /// The text of process `pid`'s `fdinfo/<fd>`.
    pub fn fdinfo(&self, pid: u32, fd: u32) -> Option<String> {
        fs::read_to_string(self.path(pid, &format!("fdinfo/{fd}"))).ok()
    }

    /// The text of process `pid`'s `status`: its name, state and memory (`VmRSS`, …).
    pub fn status(&self, pid: u32) -> Option<String> {
        fs::read_to_string(self.path(pid, "status")).ok()
    }

    /// The text of process `pid`'s `smaps_rollup`: its memory, summed over its mappings. Reading
    /// it walks the process's page tables: the bench reads it between phases only.
    pub fn smaps_rollup(&self, pid: u32) -> Option<String> {
        fs::read_to_string(self.path(pid, "smaps_rollup")).ok()
    }

    /// The environment process `pid` was started with.
    pub fn environment(&self, pid: u32) -> Option<Vec<(String, String)>> {
        let bytes = fs::read(self.path(pid, "environ")).ok()?;
        Some(parse_environment(&bytes))
    }

    /// Process `pid`'s control group, as the unified hierarchy names it: `/user.slice/…`.
    pub fn cgroup(&self, pid: u32) -> Option<String> {
        let text = fs::read_to_string(self.path(pid, "cgroup")).ok()?;
        text.lines()
            .find_map(|line| line.strip_prefix("0::"))
            .map(str::to_owned)
    }

    /// The Nix store folders of the files process `pid` maps, such as its Mesa: what it really
    /// loaded, whatever its environment asked.
    pub fn mapped_store_folders(&self, pid: u32) -> BTreeSet<String> {
        let maps = fs::read_to_string(self.path(pid, "maps")).unwrap_or_default();
        maps.lines()
            .filter_map(|line| line.split_whitespace().nth(5))
            .filter_map(store_folder)
            .map(str::to_owned)
            .collect()
    }

    /// The text of `net/unix`: every Unix socket of the network namespace, and the path it is
    /// bound to, or that its listener is bound to.
    pub fn unix_sockets(&self) -> String {
        fs::read_to_string(self.root.join("net/unix")).unwrap_or_default()
    }
}

/// The store folder `/nix/store/<hash>-<name>` that `path` lies in.
pub fn store_folder(path: &str) -> Option<&str> {
    let rest = path.strip_prefix("/nix/store/")?;
    let end = rest.find('/').unwrap_or(rest.len());
    Some(&path[..("/nix/store/".len() + end)])
}

/// The version in a store folder's name: `/nix/store/<hash>-mesa-26.2.6` gives `26.2.6`, the part
/// after the first `-` that a digit follows.
pub fn store_version(folder: &str) -> Option<&str> {
    let name = Path::new(folder).file_name()?.to_str()?;
    let (_hash, rest) = name.split_once('-')?;
    let start = rest
        .match_indices('-')
        .find(|(at, _)| rest[at + 1..].starts_with(|c: char| c.is_ascii_digit()))?
        .0;
    let version = &rest[start + 1..];
    // An output's suffix, such as `-bin` of `ffmpeg-headless-9.0.2-bin`, is not the version's.
    Some(version.split_once("-bin").map_or(version, |(v, _)| v))
}

/// The CPU time in nanoseconds of a `schedstat`: its first field.
pub fn parse_schedstat(text: &str) -> Option<u64> {
    text.split_whitespace().next()?.parse().ok()
}

/// The voluntary and involuntary context switches of a thread's `status`.
pub fn parse_context_switches(text: &str) -> Option<(u64, u64)> {
    let value = |key: &str| {
        text.lines()
            .find_map(|line| line.strip_prefix(key)?.strip_prefix(':'))
            .and_then(|value| value.trim().parse().ok())
    };
    Some((
        value("voluntary_ctxt_switches")?,
        value("nonvoluntary_ctxt_switches")?,
    ))
}

/// A process's parent and its processor time from its `stat`: `utime`, `stime`, `cutime` and
/// `cstime`, fields 14 to 17, read after the last `)`, which ends its name.
pub fn parse_stat(stat: &str) -> Option<(u32, u64)> {
    let fields: Vec<&str> = stat
        .get(stat.rfind(')')? + 1..)?
        .split_whitespace()
        .collect();
    // After the name come the state, field 3, and the parent, field 4.
    let parent = fields.get(1)?.parse().ok()?;
    let ticks = fields
        .get(11..15)?
        .iter()
        .map(|field| field.parse::<u64>().ok())
        .sum::<Option<u64>>()?;
    Some((parent, ticks))
}

/// The value of the line `key:  <n> kB` of `text`, such as `smaps_rollup`'s, in KiB.
pub fn parse_kib(text: &str, key: &str) -> Option<u64> {
    text.lines().find_map(|line| {
        let value = line.strip_prefix(key)?.strip_prefix(':')?;
        value.trim().strip_suffix("kB")?.trim().parse().ok()
    })
}

/// The variables of an `environ`: `KEY=value` strings, each ended by a NUL.
pub fn parse_environment(bytes: &[u8]) -> Vec<(String, String)> {
    bytes
        .split(|byte| *byte == 0)
        .filter(|entry| !entry.is_empty())
        .filter_map(|entry| {
            let entry = String::from_utf8_lossy(entry);
            let (key, value) = entry.split_once('=')?;
            Some((key.to_owned(), value.to_owned()))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schedstat_gives_the_time_on_a_core() {
        assert_eq!(parse_schedstat("123456789 4567 89\n"), Some(123_456_789));
        assert_eq!(parse_schedstat(""), None);
    }

    #[test]
    fn status_gives_both_kinds_of_context_switch() {
        let status = "Name:\tsway\nState:\tS (sleeping)\nvoluntary_ctxt_switches:\t1520\n\
                      nonvoluntary_ctxt_switches:\t37\n";
        assert_eq!(parse_context_switches(status), Some((1520, 37)));
        assert_eq!(parse_context_switches("Name:\tsway\n"), None);
    }

    #[test]
    fn stat_gives_the_parent_and_the_time_with_reaped_childrens() {
        // Its name holds a space and a parenthesis; utime 7, stime 3, cutime 20, cstime 5.
        let stat = "4242 (Web (Content) 1) S 4200 4242 4200 0 -1 4194560 100 0 0 0 7 3 20 5 20 0 \
                    1 0 1000 1000000 100 18446744073709551615";
        assert_eq!(parse_stat(stat), Some((4200, 35)));
        assert_eq!(parse_stat("4242 (cut"), None);
    }

    #[test]
    fn smaps_rollup_lines_are_read_in_kib() {
        let rollup = "55d0-7ffd ---p 00000000 00:00 0 [rollup]\nRss:  41236 kB\nPss:  20512 kB\n\
                      Pss_Anon:  9000 kB\nPss_File:  11000 kB\nPss_Shmem:  512 kB\n";
        assert_eq!(parse_kib(rollup, "Rss"), Some(41_236));
        assert_eq!(parse_kib(rollup, "Pss"), Some(20_512));
        assert_eq!(parse_kib(rollup, "Pss_Anon"), Some(9_000));
        assert_eq!(parse_kib(rollup, "Pss_Shmem"), Some(512));
        assert_eq!(parse_kib(rollup, "Swap"), None);
    }

    #[test]
    fn an_environment_is_split_at_its_nuls() {
        let environment = parse_environment(b"HOME=/tmp/run/home\0EMPTY=\0A=b=c\0");
        assert_eq!(
            environment,
            [
                ("HOME".to_owned(), "/tmp/run/home".to_owned()),
                ("EMPTY".to_owned(), String::new()),
                ("A".to_owned(), "b=c".to_owned()),
            ]
        );
    }

    #[test]
    fn store_folders_give_their_package_and_version() {
        let library = "/nix/store/4viq-mesa-26.2.6/lib/libgallium-26.2.6.so";
        assert_eq!(store_folder(library), Some("/nix/store/4viq-mesa-26.2.6"));
        assert_eq!(store_folder("/usr/lib/libc.so"), None);
        assert_eq!(store_version("/nix/store/4viq-mesa-26.2.6"), Some("26.2.6"));
        assert_eq!(
            store_version("/nix/store/cf0y-sway-unwrapped-1.12"),
            Some("1.12")
        );
        assert_eq!(
            store_version("/nix/store/vjxi-ffmpeg-headless-9.0.2-bin"),
            Some("9.0.2")
        );
        assert_eq!(store_version("/nix/store/abcd-source"), None);
    }

    #[test]
    fn a_fake_proc_is_read_as_the_real_one() {
        let root = crate::testing::folder("procfs");
        let task = root.join("100/task/101");
        fs::create_dir_all(&task).unwrap();
        fs::write(root.join("100/comm"), "sway\n").unwrap();
        fs::write(task.join("comm"), "sway:gdrv0\n").unwrap();
        fs::write(task.join("schedstat"), "5000 10 3\n").unwrap();
        fs::write(
            task.join("status"),
            "voluntary_ctxt_switches:\t4\nnonvoluntary_ctxt_switches:\t1\n",
        )
        .unwrap();
        fs::write(root.join("100/status"), "Name:\tsway\nVmRSS:\t   1234 kB\n").unwrap();
        fs::create_dir_all(root.join("100/fd")).unwrap();
        std::os::unix::fs::symlink("/dev/dri/renderD128", root.join("100/fd/7")).unwrap();
        let proc = Proc::at(&root);
        assert_eq!(proc.pids(), [100]);
        assert_eq!(proc.name(100).as_deref(), Some("sway"));
        assert_eq!(
            proc.threads(100),
            [ThreadSample {
                tid: 101,
                name: "sway:gdrv0".to_owned(),
                cpu_ns: 5000,
                voluntary: 4,
                involuntary: 1,
            }]
        );
        assert_eq!(
            proc.open_files(100),
            [(7, PathBuf::from("/dev/dri/renderD128"))]
        );
        assert_eq!(parse_kib(&proc.status(100).unwrap(), "VmRSS"), Some(1234));
        assert_eq!(proc.status(999), None);
        assert!(proc.threads(999).is_empty());
    }
}
