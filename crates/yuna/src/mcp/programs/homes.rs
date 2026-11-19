//! The seats' throwaway homes
//! ([ADR-0105](../../../../../docs/adr/0105-an-agents-programs-on-a-throwaway-home.md)): a
//! program an agent starts never reads or writes the user's settings, data or caches.
//!
//! Each seat's programs share a home, `seat-<id>` in the homes' folder, made at the seat's first
//! program that needs one, and readable by the user alone. `HOME` names it; `XDG_CONFIG_HOME`,
//! `XDG_DATA_HOME`, `XDG_CACHE_HOME` and `XDG_STATE_HOME` name folders inside it, where the XDG
//! base directory specification puts them by default ([`XDG_FOLDERS`]), so that a program that
//! reads `HOME` alone finds the same. `XDG_CONFIG_DIRS` and `XDG_DATA_DIRS`, the system's
//! folders, are Yuna's own (`environment.rs`). A variable the agent names in `spawn`'s `env`
//! wins; a program whose agent names all five needs no home, and none is made for it.
//!
//! The homes' folder, `yuna-mcp-homes-<pid>`, is on disk, in `TMPDIR` or else `/tmp`, never in
//! the runtime folder, a small memory-backed file system: made at the first home, locked while
//! Yuna lives, and swept as the programs' folder is (`folder.rs`). A seat's home goes when the
//! seat closes; the folder when Yuna stops, and on the fatal path, which knows the programs'
//! folder alone: a link there, [`HOMES_LINK`], names the homes' folder.
//!
//! Its neighbours: `mod.rs` asks a seat's home for each program it starts and removes it with
//! its seat; `processes.rs` removes the folder on the fatal path.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs::{self, File};
use std::io;
use std::os::unix::fs::{DirBuilderExt, MetadataExt};
use std::path::{Path, PathBuf};

use smithay::reexports::rustix;
use yuna_layout::SeatId;

use super::folder::{HOMES_FOLDER, remove_quietly};

/// The variable that names a program's home.
pub(super) const HOME_VARIABLE: &str = "HOME";

/// The XDG base folders inside a seat's home, by variable: the XDG base directory
/// specification's defaults under `HOME`.
pub(super) const XDG_FOLDERS: [(&str, &str); 4] = [
    ("XDG_CONFIG_HOME", ".config"),
    ("XDG_DATA_HOME", ".local/share"),
    ("XDG_CACHE_HOME", ".cache"),
    ("XDG_STATE_HOME", ".local/state"),
];

/// The link in the programs' folder that names the homes' folder, for the fatal path.
pub(super) const HOMES_LINK: &str = "homes";

/// A `yuna mcp`'s seats' homes.
#[derive(Debug)]
pub(super) struct Homes {
    /// Where the homes' folder goes: `TMPDIR`, or `/tmp`.
    temp_dir: PathBuf,
    /// The homes' folder and its lock, once the first home is made.
    folder: Option<(PathBuf, File)>,
}

impl Homes {
    /// The homes of a `yuna mcp` whose temporary folder is `temp_dir`; first sweeps the homes'
    /// folders that Yunas gone left there.
    pub(super) fn new(temp_dir: PathBuf) -> Homes {
        HOMES_FOLDER.remove_stale(&temp_dir);
        Homes {
            temp_dir,
            folder: None,
        }
    }

    /// Seat `seat`'s home, made with its XDG folders if it is not there yet; the homes'
    /// folder first, if it is not, with its link in `programs_folder`.
    pub(super) fn seat_home(
        &mut self,
        seat: SeatId,
        programs_folder: &Path,
    ) -> io::Result<PathBuf> {
        let folder = match &self.folder {
            Some((folder, _)) => folder.clone(),
            None => {
                let (folder, lock) = HOMES_FOLDER.make(&self.temp_dir)?;
                let link = programs_folder.join(HOMES_LINK);
                if let Err(error) = std::os::unix::fs::symlink(&folder, &link) {
                    remove_quietly(&folder);
                    return Err(error);
                }
                self.folder = Some((folder.clone(), lock));
                folder
            }
        };
        let home = folder.join(format!("seat-{}", seat.0));
        let mut builder = fs::DirBuilder::new();
        builder.mode(0o700);
        match builder.create(&home) {
            Ok(()) => {}
            // Made at the seat's first program; a program may have put a file or a link there,
            // which is no home.
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                return if fs::symlink_metadata(&home).is_ok_and(|metadata| metadata.is_dir()) {
                    Ok(home)
                } else {
                    Err(io::Error::other(format!(
                        "{} is not a folder",
                        home.display()
                    )))
                };
            }
            Err(error) => return Err(error),
        }
        builder.recursive(true);
        for (_, inner) in XDG_FOLDERS {
            if let Err(error) = builder.create(home.join(inner)) {
                remove_quietly(&home);
                return Err(error);
            }
        }
        Ok(home)
    }

    /// Removes seat `seat`'s home, which closed, and what its programs left there.
    pub(super) fn remove_seat(&self, seat: SeatId) {
        if let Some((folder, _)) = &self.folder {
            remove_quietly(&folder.join(format!("seat-{}", seat.0)));
        }
    }

    /// Removes the homes' folder, every seat's home with it: Yuna stops.
    pub(super) fn remove(self) {
        if let Some((folder, _lock)) = self.folder {
            remove_quietly(&folder);
        }
    }
}

/// Whether a program whose agent gives `added` needs its seat's home: unless it names `HOME` and
/// the four XDG base folders.
pub(super) fn needs_home(added: &BTreeMap<String, String>) -> bool {
    std::iter::once(HOME_VARIABLE)
        .chain(XDG_FOLDERS.iter().map(|(name, _)| *name))
        .any(|name| !added.contains_key(name))
}

/// Adds to a program's `environment` its seat's `home`: `HOME` and the four XDG base folders
/// inside it, each but those the agent's `added` give.
pub(super) fn add_home_variables(
    environment: &mut BTreeMap<OsString, OsString>,
    home: &Path,
    added: &BTreeMap<String, String>,
) {
    let mut add = |name: &str, value: PathBuf| {
        if !added.contains_key(name) {
            environment.insert(name.into(), value.into());
        }
    };
    add(HOME_VARIABLE, home.to_owned());
    for (name, inner) in XDG_FOLDERS {
        add(name, home.join(inner));
    }
}

/// The temporary folder Yuna was given: `TMPDIR` when it is an absolute path, else `/tmp`.
pub(super) fn temp_dir() -> PathBuf {
    std::env::var_os("TMPDIR")
        .map(PathBuf::from)
        .filter(|folder| folder.is_absolute())
        .unwrap_or_else(|| PathBuf::from("/tmp"))
}

/// Removes the homes' folder that `programs_folder`'s link names, on the fatal path, which has
/// no [`Homes`]: only when the link names this process's homes' folder, a folder of the user's,
/// so that a link a program changed never makes Yuna remove another folder.
pub(super) fn remove_linked_homes(programs_folder: &Path) {
    let Ok(folder) = fs::read_link(programs_folder.join(HOMES_LINK)) else {
        return;
    };
    let own_name = folder
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| HOMES_FOLDER.names_for(name, std::process::id()));
    let user = rustix::process::geteuid().as_raw();
    let own_folder = fs::symlink_metadata(&folder)
        .is_ok_and(|metadata| metadata.is_dir() && metadata.uid() == user);
    if folder.is_absolute() && own_name && own_folder {
        remove_quietly(&folder);
    }
}
