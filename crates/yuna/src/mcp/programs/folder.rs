//! The folders a `yuna mcp` makes for itself, of two kinds ([`FolderKind`]): its programs'
//! folder, `yuna-mcp-<pid>` in `XDG_RUNTIME_DIR`, which holds its seats' sockets, its programs'
//! logs and runtime folders; and its seats' homes' folder, `yuna-mcp-homes-<pid>` in `TMPDIR` or
//! `/tmp` (`homes.rs`). Each is made under a name of its own, its [`LOCK_FILE`] locked while Yuna
//! lives, then given its name; removed when Yuna stops. One that a Yuna killed by SIGKILL left
//! is swept when the next `yuna mcp` starts, its lock free.
//!
//! Its neighbours: `mod.rs` makes the programs' folder (`Programs::new`) and removes it;
//! `homes.rs` makes the homes' folder; `processes.rs` removes both on the fatal path.

use std::fs::{self, File, OpenOptions};
use std::io;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

use smithay::reexports::rustix;

use super::processes::any_alive;

/// The file in a `yuna mcp`'s folder whose lock it holds while it lives (`flock`, through
/// [`File::try_lock`]): the kernel releases it at any end, SIGKILL included, and in any pid
/// namespace, so that a lock another Yuna can take tells a folder whose Yuna is gone.
pub const LOCK_FILE: &str = "lock";

/// A kind of folder a `yuna mcp` makes for itself in a parent folder, `<prefix><pid>`,
/// readable by its user alone and locked while it lives.
#[derive(Debug)]
pub(super) struct FolderKind {
    /// The start of its name, before Yuna's pid.
    pub prefix: &'static str,
    /// The folders made inside it.
    pub inner: &'static [&'static str],
}

/// The folder of a `yuna mcp`'s seats' sockets, programs' logs and runtime folders, in
/// `XDG_RUNTIME_DIR`.
pub(super) const PROGRAMS_FOLDER: FolderKind = FolderKind {
    prefix: "yuna-mcp-",
    inner: &["logs", "runtime"],
};

/// The folder of a `yuna mcp`'s seats' throwaway homes, in `TMPDIR` or `/tmp`
/// ([ADR-0105](../../../../../docs/adr/0105-an-agents-programs-on-a-throwaway-home.md)).
pub(super) const HOMES_FOLDER: FolderKind = FolderKind {
    prefix: "yuna-mcp-homes-",
    inner: &[],
};

impl FolderKind {
    /// Makes this kind's folder in `parent`, its inner folders and its locked [`LOCK_FILE`];
    /// returns its path and the lock, which this Yuna holds for as long as it lives. It is made
    /// and locked under a name of its own, `.<prefix><pid>-starting`, which no sweep takes, then
    /// given its name ([`FolderKind::name_folder`]): another Yuna's sweep never finds it unlocked.
    /// Nothing is left of it when a step fails.
    pub(super) fn make(&self, parent: &Path) -> io::Result<(PathBuf, File)> {
        let pid = std::process::id();
        let staging = parent.join(format!(".{}{pid}-starting", self.prefix));
        let made = self
            .prepare_folder(&staging)
            .and_then(|lock| Ok((self.name_folder(parent, pid, &staging)?, lock)));
        if made.is_err() {
            let _ = fs::remove_dir_all(&staging);
        }
        made
    }

    /// Makes the folder `staging`, its lock file locked, and its inner folders; returns the
    /// lock. A folder of that name is a gone Yuna's that had this pid, and goes first; one
    /// another user holds, in a shared `/tmp`, cannot go, and fails it.
    fn prepare_folder(&self, staging: &Path) -> io::Result<File> {
        match fs::DirBuilder::new().mode(0o700).create(staging) {
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                fs::remove_dir_all(staging)?;
                fs::DirBuilder::new().mode(0o700).create(staging)?;
            }
            made => made?,
        }
        let lock = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(staging.join(LOCK_FILE))?;
        lock.try_lock().map_err(io::Error::from)?;
        for inner in self.inner {
            fs::DirBuilder::new()
                .mode(0o700)
                .create(staging.join(inner))?;
        }
        Ok(lock)
    }

    /// Gives `staging` its name in `parent`, `<prefix><pid>`, or `<prefix><pid>-<n>` when a
    /// folder of that name is there, a gone Yuna's whose lock is not free yet; never in place of
    /// another.
    fn name_folder(&self, parent: &Path, pid: u32, staging: &Path) -> io::Result<PathBuf> {
        let prefix = self.prefix;
        for attempt in 0..100 {
            let folder = parent.join(match attempt {
                0 => format!("{prefix}{pid}"),
                n => format!("{prefix}{pid}-{n}"),
            });
            let renamed = rustix::fs::renameat_with(
                rustix::fs::CWD,
                staging,
                rustix::fs::CWD,
                &folder,
                rustix::fs::RenameFlags::NOREPLACE,
            );
            match renamed {
                Ok(()) => return Ok(folder),
                Err(rustix::io::Errno::EXIST) => continue,
                Err(error) => return Err(error.into()),
            }
        }
        Err(io::Error::other(format!(
            "every name of {prefix}{pid}'s is taken"
        )))
    }

    /// Whether `name` is this kind's folder's: `<prefix><pid>` or `<prefix><pid>-<n>`. A
    /// folder still being made, `.<prefix><pid>-starting`, is not.
    pub(super) fn names(&self, name: &str) -> bool {
        let Some(rest) = name.strip_prefix(self.prefix) else {
            return false;
        };
        let (pid, attempt) = rest.split_once('-').unwrap_or((rest, "0"));
        let digits =
            |text: &str| !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit());
        digits(pid) && digits(attempt)
    }

    /// Whether `name` is this kind's folder's for the process `pid`.
    pub(super) fn names_for(&self, name: &str, pid: u32) -> bool {
        let own = format!("{}{pid}", self.prefix);
        self.names(name)
            && name
                .strip_prefix(&own)
                .is_some_and(|rest| rest.is_empty() || rest.starts_with('-'))
    }

    /// Removes each of this kind's folders in `parent` whose [`LOCK_FILE`]'s lock can be taken:
    /// the folder a Yuna killed by SIGKILL left, which nothing else removes. It is removed with
    /// its lock held, so that no other sweep takes it meanwhile; never by its pid alone. A link
    /// is not followed; a folder another user owns, in a `/tmp` every user shares, is left; and
    /// a folder with no lock file, a Yuna's older than its lock, is kept: nothing tells whether
    /// it lives.
    pub(super) fn remove_stale(&self, parent: &Path) {
        let Ok(entries) = fs::read_dir(parent) else {
            return;
        };
        let user = rustix::process::geteuid().as_raw();
        for entry in entries.flatten() {
            // The entry's own metadata: a link to a folder is not followed.
            let ours = entry
                .metadata()
                .is_ok_and(|metadata| metadata.is_dir() && metadata.uid() == user);
            if !ours
                || !entry
                    .file_name()
                    .to_str()
                    .is_some_and(|name| self.names(name))
            {
                continue;
            }
            let folder = entry.path();
            let Ok(lock) = File::open(folder.join(LOCK_FILE)) else {
                continue;
            };
            match lock.try_lock() {
                Ok(()) => match fs::remove_dir_all(&folder) {
                    Ok(()) => tracing::info!(?folder, "a gone Yuna's folder is removed"),
                    Err(error) => {
                        tracing::warn!(%error, ?folder, "a gone Yuna's folder cannot be removed");
                    }
                },
                // Its Yuna lives.
                Err(std::fs::TryLockError::WouldBlock) => {}
                Err(std::fs::TryLockError::Error(error)) => {
                    tracing::warn!(%error, ?folder, "a Yuna's folder's lock cannot be read");
                }
            }
        }
    }
}

/// Removes `folder`, the programs' folder, warning when a process under Yuna outlives it. A
/// folder already gone, by a panic after Yuna's stopping removed it, is no failure.
pub(super) fn remove_folder(folder: &Path) {
    if any_alive() {
        tracing::warn!("programs outlive Yuna");
    }
    remove_quietly(folder);
}

/// Removes `folder` and what it holds, warning when it cannot: one already gone is no failure.
pub(super) fn remove_quietly(folder: &Path) {
    match fs::remove_dir_all(folder) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => tracing::warn!(%error, ?folder, "a folder of Yuna's cannot be removed"),
    }
}
