//! The folder of a `yuna mcp`'s seats' sockets, programs' logs and runtime folders,
//! `yuna-mcp-<pid>` in `XDG_RUNTIME_DIR`: made under a name of its own, its [`LOCK_FILE`] locked
//! while Yuna lives, then given its name; removed when Yuna stops. One that a Yuna killed by
//! SIGKILL left is swept when the next `yuna mcp` starts, its lock free.
//!
//! Its neighbours: `mod.rs` makes it (`Programs::new`) and removes it; `processes.rs` removes it
//! on the fatal path.

use std::fs::{self, File, OpenOptions};
use std::io;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

use smithay::reexports::rustix;

use super::processes::any_alive;

/// The prefix of the folders `yuna mcp` makes in `XDG_RUNTIME_DIR`, before its pid.
pub(super) const FOLDER_PREFIX: &str = "yuna-mcp-";

/// The file in a `yuna mcp`'s folder whose lock it holds while it lives (`flock`, through
/// [`File::try_lock`]): the kernel releases it at any end, SIGKILL included, and in any pid
/// namespace, so that a lock another Yuna can take tells a folder whose Yuna is gone.
pub const LOCK_FILE: &str = "lock";

/// Makes the folder `staging`, its lock file locked, and its `logs` and `runtime` folders;
/// returns the lock. A folder of that name is a gone Yuna's that had this pid, and goes first.
pub(super) fn prepare_folder(staging: &Path) -> io::Result<File> {
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
    for inner in ["logs", "runtime"] {
        fs::DirBuilder::new()
            .mode(0o700)
            .create(staging.join(inner))?;
    }
    Ok(lock)
}

/// Gives `staging` its name in `runtime_dir`, `yuna-mcp-<pid>`, or `yuna-mcp-<pid>-<n>` when
/// a folder of that name is there, a gone Yuna's whose lock is not free yet; never in place of
/// another.
pub(super) fn name_folder(runtime_dir: &Path, pid: u32, staging: &Path) -> io::Result<PathBuf> {
    for attempt in 0..100 {
        let folder = runtime_dir.join(match attempt {
            0 => format!("{FOLDER_PREFIX}{pid}"),
            n => format!("{FOLDER_PREFIX}{pid}-{n}"),
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
        "every name of {FOLDER_PREFIX}{pid}'s is taken"
    )))
}

/// Whether `name` is a `yuna mcp`'s folder's: `yuna-mcp-<pid>` or `yuna-mcp-<pid>-<n>`. A
/// folder still being made, `.yuna-mcp-<pid>-starting`, is not.
pub(super) fn is_folder_name(name: &str) -> bool {
    let Some(rest) = name.strip_prefix(FOLDER_PREFIX) else {
        return false;
    };
    let (pid, attempt) = rest.split_once('-').unwrap_or((rest, "0"));
    let digits = |text: &str| !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit());
    digits(pid) && digits(attempt)
}

/// Removes each `yuna mcp`'s folder in `runtime_dir` whose [`LOCK_FILE`]'s lock can be taken:
/// the folder of sockets, logs and runtime folders a Yuna killed by SIGKILL left, which nothing
/// else removes. It is removed with its lock held, so that no other sweep takes it meanwhile;
/// never by its pid alone. A link is not followed, and a folder with no lock file, a Yuna's
/// older than this one, is kept: nothing tells whether it lives.
pub(super) fn remove_stale_folders(runtime_dir: &Path) {
    let Ok(entries) = fs::read_dir(runtime_dir) else {
        return;
    };
    for entry in entries.flatten() {
        // The entry's own type: a link to a folder is not followed.
        let is_folder = entry.file_type().is_ok_and(|kind| kind.is_dir());
        if !is_folder || !entry.file_name().to_str().is_some_and(is_folder_name) {
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

/// Removes `folder`, the programs' folder, warning when a process under Yuna outlives it. A
/// folder already gone, by a panic after Yuna's stopping removed it, is no failure.
pub(super) fn remove_folder(folder: &Path) {
    if any_alive() {
        tracing::warn!("programs outlive Yuna");
    }
    match fs::remove_dir_all(folder) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => tracing::warn!(%error, ?folder, "the programs' folder cannot be removed"),
    }
}
