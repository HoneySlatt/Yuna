//! The folders a `yuna mcp` makes for itself, of two kinds ([`FolderKind`]): its programs'
//! folder, `yuna-mcp-<pid>` in `XDG_RUNTIME_DIR`, which holds its seats' sockets, its programs'
//! logs and runtime folders; and its seats' homes' folder, `yuna-mcp-homes-<pid>` in `TMPDIR` or
//! `/tmp` (`homes.rs`). Each is made under a name of its own, its [`LOCK_FILE`] locked while Yuna
//! lives, then given its name; removed when Yuna stops. One that a Yuna killed by SIGKILL left
//! is swept when the next `yuna mcp` starts, its lock free. In a `/tmp` every user shares,
//! another user may take a name Yuna would take, which Yuna cannot remove: a name that holds
//! Yuna's pid alone can be foreseen, so that the name a folder is made under holds a random
//! number before its `-starting`, and the one it takes when its own is taken ends with one.
//!
//! Its neighbours: `mod.rs` makes the programs' folder (`Programs::new`) and removes it;
//! `homes.rs` makes the homes' folder; `processes.rs` removes both on the fatal path.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

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
    /// and locked under a name of its own, `.<prefix><pid>-<random>-starting`, which no sweep
    /// takes, then given its name ([`FolderKind::name_folder`]): another Yuna's sweep never finds
    /// it unlocked. A folder of that name already there is another's and fails it, untouched;
    /// nothing is left of Yuna's own when a later step fails.
    pub(super) fn make(&self, parent: &Path) -> io::Result<(PathBuf, File)> {
        let pid = std::process::id();
        let staging = parent.join(format!(
            ".{}{pid}-{}-starting",
            self.prefix,
            random_number()?
        ));
        fs::DirBuilder::new().mode(0o700).create(&staging)?;
        let made = self
            .prepare_folder(&staging)
            .and_then(|lock| Ok((self.name_folder(parent, pid, &staging)?, lock)));
        if made.is_err() {
            let _ = fs::remove_dir_all(&staging);
        }
        made
    }

    /// Makes `staging`'s lock file locked, and its inner folders; returns the lock.
    fn prepare_folder(&self, staging: &Path) -> io::Result<File> {
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

    /// Gives `staging` its name in `parent`, `<prefix><pid>`, never in place of another. When a
    /// folder of that name is there, a gone Yuna's whose lock is not free yet, or another user's
    /// in a shared `/tmp`, it takes `<prefix><pid>-<random>`, which no one can take before it.
    fn name_folder(&self, parent: &Path, pid: u32, staging: &Path) -> io::Result<PathBuf> {
        let prefix = self.prefix;
        let own = parent.join(format!("{prefix}{pid}"));
        match rename_without_replacing(staging, &own) {
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            renamed => return renamed.map(|()| own),
        }
        let other = parent.join(format!("{prefix}{pid}-{}", random_number()?));
        rename_without_replacing(staging, &other)?;
        Ok(other)
    }

    /// Whether `name` is this kind's folder's: `<prefix><pid>` or `<prefix><pid>-<n>`. A
    /// folder still being made, `.<prefix><pid>-<n>-starting`, is not.
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
            // The name first, which the directory's listing gives: `parent` may be a `/tmp` of
            // tens of thousands of entries, and reading each one's metadata cost `yuna mcp`'s
            // start 60 ms there (M2.1).
            if !entry
                .file_name()
                .to_str()
                .is_some_and(|name| self.names(name))
            {
                continue;
            }
            // The entry's own metadata: a link to a folder is not followed.
            let ours = entry
                .metadata()
                .is_ok_and(|metadata| metadata.is_dir() && metadata.uid() == user);
            if !ours {
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

/// Renames `from` to `to`, refused when `to` is there.
fn rename_without_replacing(from: &Path, to: &Path) -> io::Result<()> {
    rustix::fs::renameat_with(
        rustix::fs::CWD,
        from,
        rustix::fs::CWD,
        to,
        rustix::fs::RenameFlags::NOREPLACE,
    )
    .map_err(io::Error::from)
}

/// A number no other process can foresee, from the kernel's random source.
fn random_number() -> io::Result<u64> {
    let mut bytes = [0; 8];
    File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    Ok(u64::from_ne_bytes(bytes))
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
