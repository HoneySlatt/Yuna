//! A control group of the unified hierarchy, `/sys/fs/cgroup`: the CPU time of every process that
//! ever ran in it, those that ended and their threads included (`cpu.stat`), and the processes it
//! holds now.
//!
//! Each compositor runs in a transient systemd scope of its own (`sandbox`); its own process and
//! its descendants move into a child group, `core`. The scope's `cpu.stat` is then the stack's
//! time, the child's the compositor's, both exact over any window.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// Where the unified hierarchy is mounted.
pub const ROOT: &str = "/sys/fs/cgroup";

/// One control group, by its folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cgroup {
    folder: PathBuf,
}

/// A group's CPU time so far, from its `cpu.stat`, in microseconds.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CpuStat {
    /// User and system time together.
    pub usage_us: u64,
    /// In user space.
    pub user_us: u64,
    /// In the kernel, on the group's behalf.
    pub system_us: u64,
}

impl Cgroup {
    /// The group the hierarchy names `name`, such as `/user.slice/…/app.slice/x.scope`, under
    /// `root`, `/sys/fs/cgroup` but in tests.
    pub fn named(root: &Path, name: &str) -> Cgroup {
        Cgroup {
            folder: root.join(name.trim_start_matches('/')),
        }
    }

    /// Its folder.
    pub fn folder(&self) -> &Path {
        &self.folder
    }

    /// Its child group `name`.
    pub fn child(&self, name: &str) -> Cgroup {
        Cgroup {
            folder: self.folder.join(name),
        }
    }

    /// Its CPU time so far, its children's included; none once it is gone.
    pub fn cpu(&self) -> Option<CpuStat> {
        parse_cpu_stat(&fs::read_to_string(self.folder.join("cpu.stat")).ok()?)
    }

    /// Every process it and its children hold now.
    pub fn processes(&self) -> Vec<u32> {
        let mut pids = Vec::new();
        collect_processes(&self.folder, &mut pids);
        pids.sort_unstable();
        pids.dedup();
        pids
    }

    /// Whether it holds a process still, in itself or a child; false once it is gone.
    pub fn is_populated(&self) -> bool {
        fs::read_to_string(self.folder.join("cgroup.events"))
            .ok()
            .is_some_and(|events| events.lines().any(|line| line == "populated 1"))
    }

    /// Kills every process it and its children hold, at once, with SIGKILL (`cgroup.kill`).
    pub fn kill(&self) -> io::Result<()> {
        fs::write(self.folder.join("cgroup.kill"), "1")
    }
}

fn collect_processes(folder: &Path, pids: &mut Vec<u32>) {
    if let Ok(text) = fs::read_to_string(folder.join("cgroup.procs")) {
        pids.extend(
            text.lines()
                .filter_map(|line| line.trim().parse::<u32>().ok()),
        );
    }
    for entry in fs::read_dir(folder).into_iter().flatten().flatten() {
        if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            collect_processes(&entry.path(), pids);
        }
    }
}

/// The times of a `cpu.stat`.
pub fn parse_cpu_stat(text: &str) -> Option<CpuStat> {
    let value = |key: &str| {
        text.lines().find_map(|line| {
            let (name, value) = line.split_once(' ')?;
            (name == key).then(|| value.trim().parse().ok())?
        })
    };
    Some(CpuStat {
        usage_us: value("usage_usec")?,
        user_us: value("user_usec")?,
        system_us: value("system_usec")?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cpu_stat_gives_its_three_times() {
        let text = "usage_usec 12428\nuser_usec 5784\nsystem_usec 6643\nnr_periods 0\n\
                    core_sched.force_idle_usec 0\n";
        assert_eq!(
            parse_cpu_stat(text),
            Some(CpuStat {
                usage_us: 12_428,
                user_us: 5_784,
                system_us: 6_643,
            })
        );
        assert_eq!(parse_cpu_stat("nr_periods 0\n"), None);
    }

    #[test]
    fn a_group_lists_its_childrens_processes_too() {
        let root = crate::testing::folder("cgroup");
        let scope = Cgroup::named(&root, "/user.slice/bench.scope");
        fs::create_dir_all(scope.child("core").folder()).unwrap();
        fs::write(scope.folder().join("cgroup.procs"), "30\n31\n").unwrap();
        fs::write(scope.child("core").folder().join("cgroup.procs"), "40\n").unwrap();
        fs::write(
            scope.folder().join("cgroup.events"),
            "populated 1\nfrozen 0\n",
        )
        .unwrap();
        assert_eq!(scope.processes(), [30, 31, 40]);
        assert_eq!(scope.child("core").processes(), [40]);
        assert!(scope.is_populated());
        assert!(!scope.child("gone").is_populated());
        assert_eq!(scope.child("gone").cpu(), None);
    }
}
