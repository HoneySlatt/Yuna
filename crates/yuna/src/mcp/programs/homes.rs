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
//! Yuna lives, and swept as the programs' folder is (`folder.rs`). A cleaner of the temporary
//! folder, such as systemd-tmpfiles' age rule, may remove it while Yuna runs: it is made again,
//! under the same rules, at the next home; a folder another process put at its name is never
//! taken for Yuna's own. A seat's home goes when the seat closes; the folder when Yuna stops,
//! and on the fatal path, which knows the programs' folder alone: a link there, [`HOMES_LINK`],
//! names the homes' folder, and is followed only to a folder of that name in the temporary
//! folder.
//!
//! Its neighbours: `mod.rs` asks a seat's home for each program it starts and removes it with
//! its seat; `processes.rs` removes the folder on the fatal path.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs::{self, File};
use std::io;
use std::os::unix::fs::{DirBuilderExt, MetadataExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use smithay::reexports::rustix;
use smithay::reexports::rustix::fs::{Mode, OFlags};
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
pub const HOMES_LINK: &str = "homes";

/// A `yuna mcp`'s seats' homes.
#[derive(Debug)]
pub(super) struct Homes {
    /// Where the homes' folder goes: `TMPDIR`, or `/tmp`.
    temp_dir: PathBuf,
    /// The homes' folder, once the first home is made.
    folder: Option<HomesFolder>,
}

/// The device and inode of the homes' folder this process made last, 0 before any, for the fatal
/// path, which has no [`Homes`] and must remove that folder alone: not another put at its name
/// once it was gone, nor one a program's link names.
static MADE_DEVICE: AtomicU64 = AtomicU64::new(0);
static MADE_INODE: AtomicU64 = AtomicU64::new(0);

/// The homes' folder Yuna made.
#[derive(Debug)]
struct HomesFolder {
    path: PathBuf,
    /// The folder itself, held open: its inode stays its own while Yuna lives, even once the
    /// folder is removed, so that a folder another process puts at its path is never taken for it.
    held: File,
    /// Its lock file, locked while Yuna lives (`folder.rs`).
    _lock: File,
}

impl HomesFolder {
    /// Makes the homes' folder in `temp_dir`, and points `programs_folder`'s [`HOMES_LINK`] at
    /// it; nothing is left of it when a step fails.
    fn make(temp_dir: &Path, programs_folder: &Path) -> io::Result<HomesFolder> {
        let (path, lock) = HOMES_FOLDER.make(temp_dir)?;
        let held = open_folder(&path).and_then(|held| {
            link_homes(&path, programs_folder)?;
            Ok(held)
        });
        match held {
            Ok(held) => {
                let made = held.metadata()?;
                MADE_DEVICE.store(made.dev(), Ordering::Relaxed);
                MADE_INODE.store(made.ino(), Ordering::Relaxed);
                Ok(HomesFolder {
                    path,
                    held,
                    _lock: lock,
                })
            }
            Err(error) => {
                remove_quietly(&path);
                Err(error)
            }
        }
    }

    /// Whether the folder at its path is still the one Yuna made: not once it was removed, or
    /// another put in its place.
    fn in_place(&self) -> io::Result<bool> {
        let held = self.held.metadata()?;
        match fs::symlink_metadata(&self.path) {
            Ok(found) => Ok(found.dev() == held.dev() && found.ino() == held.ino()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(error),
        }
    }
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
    /// folder first, with its link in `programs_folder`, if it is not ([`Homes::folder`]).
    pub(super) fn seat_home(
        &mut self,
        seat: SeatId,
        programs_folder: &Path,
    ) -> io::Result<PathBuf> {
        let home = self
            .folder(programs_folder)?
            .join(format!("seat-{}", seat.0));
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

    /// The homes' folder's path: made at the first home, and made again, under the same rules,
    /// when the one Yuna made is no longer at its path, removed by a cleaner of the temporary
    /// folder. A folder another process put at that path is left to it: the new one takes
    /// another name (`folder.rs`), which `programs_folder`'s link then names.
    fn folder(&mut self, programs_folder: &Path) -> io::Result<PathBuf> {
        if let Some(folder) = &self.folder {
            if folder.in_place()? {
                return Ok(folder.path.clone());
            }
            tracing::warn!(
                folder = ?folder.path,
                "the seats' homes' folder is gone: it is made again"
            );
            self.folder = None;
        }
        let folder = HomesFolder::make(&self.temp_dir, programs_folder)?;
        let path = folder.path.clone();
        self.folder = Some(folder);
        Ok(path)
    }

    /// The homes' folder Yuna made, while it is at its path.
    fn folder_in_place(&self) -> Option<&Path> {
        self.folder
            .as_ref()
            .filter(|folder| folder.in_place().unwrap_or(false))
            .map(|folder| folder.path.as_path())
    }

    /// Removes seat `seat`'s home, which closed, and what its programs left there.
    pub(super) fn remove_seat(&self, seat: SeatId) {
        if let Some(folder) = self.folder_in_place() {
            remove_quietly(&folder.join(format!("seat-{}", seat.0)));
        }
    }

    /// Removes the homes' folder, every seat's home with it: Yuna stops.
    pub(super) fn remove(self) {
        if let Some(folder) = self.folder_in_place() {
            remove_quietly(folder);
        }
    }
}

/// Opens `folder`, which a link at its path is not.
fn open_folder(folder: &Path) -> io::Result<File> {
    let flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
    let opened = rustix::fs::open(folder, flags, Mode::empty())?;
    Ok(File::from(opened))
}

/// Points `programs_folder`'s [`HOMES_LINK`] at the homes' `folder`, in place of the link to a
/// folder made before.
fn link_homes(folder: &Path, programs_folder: &Path) -> io::Result<()> {
    let link = programs_folder.join(HOMES_LINK);
    if let Err(error) = fs::remove_file(&link)
        && error.kind() != io::ErrorKind::NotFound
    {
        return Err(error);
    }
    std::os::unix::fs::symlink(folder, &link)
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
/// no [`Homes`]: only the very folder this process made, its device and inode those it kept, of
/// this process's homes' folder's name, in the temporary folder Yuna chose, so that a link a
/// program changed, or a folder another put at the name once Yuna's was gone, never makes Yuna
/// remove another folder. Yuna never changes its own environment: [`temp_dir`] read again is the folder
/// the homes' folder was made in.
pub(super) fn remove_linked_homes(programs_folder: &Path) {
    let Ok(target) = fs::read_link(programs_folder.join(HOMES_LINK)) else {
        return;
    };
    let temp_dir = temp_dir();
    let Some(name) = target
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| HOMES_FOLDER.names_for(name, std::process::id()))
    else {
        return;
    };
    if target.parent() != Some(temp_dir.as_path()) {
        return;
    }
    // Made again from its parts, not taken from the link's text, which may end in a slash: the
    // system then follows a link at that name, to a folder of the user's elsewhere.
    let folder = temp_dir.join(name);
    let user = rustix::process::geteuid().as_raw();
    let made = (
        MADE_DEVICE.load(Ordering::Relaxed),
        MADE_INODE.load(Ordering::Relaxed),
    );
    if fs::symlink_metadata(&folder).is_ok_and(|metadata| {
        metadata.is_dir() && metadata.uid() == user && (metadata.dev(), metadata.ino()) == made
    }) {
        remove_quietly(&folder);
    }
}
