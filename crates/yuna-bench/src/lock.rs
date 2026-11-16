//! The agents' lock of exclusive runs: one timed measure at a time on the machine
//! ([ADR-0060](../../../docs/adr/0060-development-first-agents-in-their-own-yuna.md)).
//!
//! It is the file the delegate skill's `run.sh --exclusive` locks, `flock` on
//! `$OWNER_RUNTIME_DIR/agents-exclusive-run.lock` (`$XDG_RUNTIME_DIR`, then `/tmp`, without it).
//! The bench takes it itself before it measures. Started by `run.sh --exclusive`, it inherits the
//! descriptor its parent holds the lock on: it then knows the lock is held for it, and does not
//! wait for itself.

use std::fs::{self, File, OpenOptions, TryLockError};
use std::os::unix::fs::MetadataExt;
use std::path::PathBuf;

/// The lock, held until dropped.
#[derive(Debug)]
pub struct Exclusive {
    /// How it is held, for the run's file.
    pub how: String,
    _file: Option<File>,
}

/// The lock's file.
pub fn path() -> PathBuf {
    let folder = std::env::var_os("OWNER_RUNTIME_DIR")
        .or_else(|| std::env::var_os("XDG_RUNTIME_DIR"))
        .map_or_else(|| PathBuf::from("/tmp"), PathBuf::from);
    folder.join("agents-exclusive-run.lock")
}

/// Takes the lock, waiting for any other exclusive run to end; says so on standard error while
/// it waits.
pub fn take() -> Result<Exclusive, String> {
    let path = path();
    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    let identity = file
        .metadata()
        .map_err(|e| format!("{}: {e}", path.display()))?;
    let inherited = fs::read_dir("/proc/self/fd")
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| fs::metadata(entry.path()).ok())
        .filter(|other| other.dev() == identity.dev() && other.ino() == identity.ino())
        .count()
        // The file just opened is one of them.
        > 1;
    match file.try_lock() {
        Ok(()) => Ok(Exclusive {
            how: format!("taken by the bench, {}", path.display()),
            _file: Some(file),
        }),
        Err(TryLockError::WouldBlock) if inherited => Ok(Exclusive {
            how: format!("held for the bench by its parent, {}", path.display()),
            _file: None,
        }),
        Err(TryLockError::WouldBlock) => {
            eprintln!(
                "yuna-bench: waiting for another exclusive run to end ({})",
                path.display()
            );
            file.lock()
                .map_err(|e| format!("{}: {e}", path.display()))?;
            Ok(Exclusive {
                how: format!("taken by the bench after a wait, {}", path.display()),
                _file: Some(file),
            })
        }
        Err(TryLockError::Error(error)) => Err(format!("{}: {error}", path.display())),
    }
}
