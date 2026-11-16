//! The agents' lock of exclusive runs: one timed measure at a time on the machine
//! ([ADR-0060](../../../docs/adr/0060-development-first-agents-in-their-own-yuna.md)).
//!
//! It is `agents-exclusive-run.lock` in the owner's runtime folder: `$OWNER_RUNTIME_DIR`, or the
//! user's `/run/user/<uid>` when that is unset or empty. Never `$XDG_RUNTIME_DIR`: an agents'
//! service has a runtime folder of its own there, and a lock in it would hold off no agent of
//! another service. The delegate skill's `run.sh --exclusive` locks the same file when
//! `OWNER_RUNTIME_DIR` is set. The bench takes the lock itself before it measures. Started by a
//! parent that holds it, it inherits the descriptor the lock is held on: it then knows the lock
//! is held for it, and does not wait for itself. A parent that locked another file holds off
//! nothing the bench must wait for: the bench then takes this one itself.

use std::ffi::OsString;
use std::fs::{self, File, OpenOptions, TryLockError};
use std::os::unix::fs::MetadataExt;
use std::path::PathBuf;

use crate::sandbox;

/// The lock, held until dropped.
#[derive(Debug)]
pub struct Exclusive {
    /// How it is held, and which file, for the run's file.
    pub how: String,
    _file: Option<File>,
}

/// The lock's file, and where its folder came from, for the run's file.
pub fn path() -> (PathBuf, String) {
    path_from(
        std::env::var_os("OWNER_RUNTIME_DIR"),
        sandbox::user_runtime_folder(),
    )
}

/// The lock's file in `owner_runtime_dir`, else in `user_runtime_folder`; and which it is.
fn path_from(
    owner_runtime_dir: Option<OsString>,
    user_runtime_folder: PathBuf,
) -> (PathBuf, String) {
    const NAME: &str = "agents-exclusive-run.lock";
    match owner_runtime_dir.filter(|folder| !folder.is_empty()) {
        Some(folder) => (
            PathBuf::from(folder).join(NAME),
            "the folder OWNER_RUNTIME_DIR names".to_owned(),
        ),
        None => (
            user_runtime_folder.join(NAME),
            "the user's runtime folder, OWNER_RUNTIME_DIR being unset".to_owned(),
        ),
    }
}

/// Takes the lock, waiting for any other exclusive run to end; says so on standard error while
/// it waits.
pub fn take() -> Result<Exclusive, String> {
    let (path, from) = path();
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
    let held = |how: &str| format!("{how}, {}, in {from}", path.display());
    match file.try_lock() {
        Ok(()) => Ok(Exclusive {
            how: held("taken by the bench"),
            _file: Some(file),
        }),
        Err(TryLockError::WouldBlock) if inherited => Ok(Exclusive {
            how: held("held for the bench by its parent"),
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
                how: held("taken by the bench after a wait"),
                _file: Some(file),
            })
        }
        Err(TryLockError::Error(error)) => Err(format!("{}: {error}", path.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_lock_is_in_the_owners_runtime_folder_never_an_agents_service() {
        let user = PathBuf::from("/run/user/1000");
        let (path, from) = path_from(None, user.clone());
        assert_eq!(
            path,
            PathBuf::from("/run/user/1000/agents-exclusive-run.lock")
        );
        assert!(from.contains("OWNER_RUNTIME_DIR being unset"), "{from}");
        // Empty, as `run.sh`'s `${OWNER_RUNTIME_DIR:-…}` reads it: unset.
        assert_eq!(path_from(Some(OsString::new()), user.clone()).0, path);
        let (named, from) = path_from(Some(OsString::from("/srv/owner")), user);
        assert_eq!(named, PathBuf::from("/srv/owner/agents-exclusive-run.lock"));
        assert!(from.contains("OWNER_RUNTIME_DIR names"), "{from}");
    }
}
