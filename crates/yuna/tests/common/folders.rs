//! The folder a test that runs a `yuna` process of its own works in, shared by the test binaries
//! that start `yuna` themselves.

use std::fs;
use std::os::unix::fs::DirBuilderExt;
use std::path::PathBuf;

/// A folder for the tests' runtime folders short enough that the sockets Yuna makes under it,
/// an accessibility bus's the longest, fit a Unix socket's 107 bytes: CI's `TMPDIR` is too long.
fn short_temp_dir() -> PathBuf {
    let temp = std::env::temp_dir();
    if temp.as_os_str().len() <= 16 {
        temp
    } else {
        PathBuf::from("/tmp")
    }
}

/// An empty folder for test `name`, short, named after this process.
///
/// A folder a failed run kept is removed first: a process id that comes round again gives this
/// run its name, and what the old run left there, a profile or a lock, would be taken for this
/// run's.
pub fn fresh_folder(name: &str) -> PathBuf {
    let folder = short_temp_dir().join(format!("yuna-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&folder);
    // Wayland asks that a runtime folder be the user's alone. Not recursive: a folder the removal
    // left, another user's in a shared `/tmp`, fails here rather than being used as it is.
    fs::DirBuilder::new()
        .mode(0o700)
        .create(&folder)
        .expect("a test's folder");
    folder
}
