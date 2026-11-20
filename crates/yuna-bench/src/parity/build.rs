//! The two builds the parity script compares, each from its own tree, in its own development
//! shell.
//!
//! A revision is checked out as a git worktree in the work folder; a path (`--base-path`,
//! `--head-path`) is built as it is, uncommitted changes included. Each side's shell is the
//! default shell of its own `flake.nix` (`nix develop <tree>`), so that each builds with its own
//! toolchain and runs on its own Mesa, both recorded. Each is built for release with line tables
//! (`CARGO_PROFILE_RELEASE_DEBUG=line-tables-only`, which changes no code but names it in the
//! profiles) in a target folder of its own, the paths of its tree and of its target folder
//! remapped; then its programs are copied out of the target folder. One tree built twice gives the
//! same binary, byte for byte. Two trees of one revision give the same code but not the same file:
//! the development shell's linker wrapper writes the tree's own run path, `<tree>/outputs/out/lib`,
//! into the binary. So the self control compares the builds' `.text` sections, their code
//! ([`text_digest`]).
//!
//! A revision's worktree, kept in a reused work folder, is used again only if it is still at that
//! revision with no change; otherwise it is removed and checked out again.
//!
//! `yuna` is built with rustix's `use-libc-auxv`: rustix then reads the auxiliary vector through
//! glibc, not through `prctl(PR_GET_AUXV)`, which valgrind passes to the kernel and which gives
//! valgrind's own vector, whose vDSO is not in the process. The workspace sets the feature since
//! M2.1; the flag lets valgrind run the revisions before it too.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde::{Deserialize, Serialize};

use crate::procfs;

/// A program a side may have, beside `yuna`, and where its source must be for it to exist.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Example {
    /// The program's name, an example of `package`.
    pub name: &'static str,
    /// Its package.
    pub package: &'static str,
    /// Its source, from the tree's root.
    pub source: &'static str,
}

/// The layout's and motion's operations, counted by callgrind (`operations.rs`).
pub const EXAMPLES: [Example; 2] = [
    Example {
        name: "operations",
        package: "yuna-layout",
        source: "crates/yuna-layout/examples/operations.rs",
    },
    Example {
        name: "motion_operations",
        package: "yuna-motion",
        source: "crates/yuna-motion/examples/motion_operations.rs",
    },
];

/// Where a side comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// A revision git names: a commit, a branch, `HEAD`.
    Revision(String),
    /// A folder, built as it is.
    Path(PathBuf),
}

/// The canary a head is built with ([ADR-0142](../../../../docs/adr/0142-parity-a-pinned-baseline-figures-in-classes-three-verdicts.md)'s
/// controls), and the figure each must flag.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Canary {
    /// Yuna's own instructions each frame: 1.8 times the threshold of Yuna's own figure in
    /// `motion`, the widest, and more elsewhere.
    Spin,
    /// Yuna's own instructions each frame: twice the threshold of the total's figure in `full`,
    /// the widest of the scenarios that draw, and more elsewhere.
    SpinTotal,
    /// One allocation each frame, freed.
    Allocate,
    /// 64 bytes kept every 16 frames, 4 bytes a frame.
    Leak,
    /// 1 MiB kept and touched at start.
    Memory,
    /// A timer each second, at rest too.
    Wake,
    /// One more wake of the event loop each frame, after a sleep: 20 times the threshold of a
    /// frame's wakes, 5 times that of its context switches.
    FrameWake,
    /// Work in each round of the event loop.
    Round,
    /// Work in one operation of the layout.
    Layout,
    /// Work in motion's animated update.
    Motion,
}

impl Canary {
    /// Every canary.
    pub const ALL: [Canary; 10] = [
        Canary::Spin,
        Canary::SpinTotal,
        Canary::Allocate,
        Canary::Leak,
        Canary::Memory,
        Canary::Wake,
        Canary::FrameWake,
        Canary::Round,
        Canary::Layout,
        Canary::Motion,
    ];

    /// Its name, as `--canary` and `YUNA_PARITY_CANARY` take it.
    pub fn name(self) -> &'static str {
        match self {
            Canary::Spin => "spin",
            Canary::SpinTotal => "spin-total",
            Canary::Allocate => "allocate",
            Canary::Leak => "leak",
            Canary::Memory => "memory",
            Canary::Wake => "wake",
            Canary::FrameWake => "frame-wake",
            Canary::Round => "round",
            Canary::Layout => "layout",
            Canary::Motion => "motion",
        }
    }

    /// The canary named `name`.
    pub fn named(name: &str) -> Option<Canary> {
        Canary::ALL.into_iter().find(|canary| canary.name() == name)
    }

    /// The package whose `parity-canary` feature turns it on.
    pub fn package(self) -> &'static str {
        match self {
            Canary::Layout => "yuna-layout",
            Canary::Motion => "yuna-motion",
            _ => "yuna",
        }
    }
}

/// What the run file records of a side.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SideInfo {
    /// `base` or `head`.
    pub side: String,
    /// The revision asked, or the folder.
    pub asked: String,
    /// Its commit; for a folder, its `HEAD`'s.
    pub commit: Option<String>,
    /// For a folder: whether its tree differs from its `HEAD`.
    pub dirty: Option<bool>,
    /// The toolchain of its shell, `rustc -V`.
    pub rustc: Option<String>,
    /// The Mesa of its shell: the store folder its EGL vendor file is in.
    pub mesa: Option<String>,
    /// The canary it was built with.
    pub canary: Option<String>,
    /// Each program it built, and its digest (FNV-1a, 64 bits, of the file); the programs its
    /// tree lacks are not listed.
    pub programs: BTreeMap<String, String>,
    /// Each program's digest of its `.text` section alone, its code.
    #[serde(default)]
    pub code: BTreeMap<String, String>,
    /// Why it could not be built.
    pub error: Option<String>,
}

/// A side, built.
#[derive(Debug, Clone)]
pub struct Built {
    /// What the run file records.
    pub info: SideInfo,
    /// Each program it has, copied out: `yuna`, and those of [`EXAMPLES`] its tree has.
    pub programs: BTreeMap<String, PathBuf>,
    /// The cases each of its operations programs lists, or why it could not list them.
    pub cases: BTreeMap<String, Result<Vec<String>, String>>,
    /// Its tree's thresholds file, read; none if the tree has none.
    pub thresholds: Option<Result<super::thresholds::Thresholds, String>>,
    /// The variables of its shell Yuna reads.
    pub run_environment: Vec<(String, String)>,
}

/// The variables of a side's shell given to its Yuna: its Mesa through libglvnd, its keymaps,
/// its D-Bus daemon, and its `PATH`, where Xwayland is.
const RUN_VARIABLES: [&str; 5] = [
    "PATH",
    "LD_LIBRARY_PATH",
    "__EGL_VENDOR_LIBRARY_FILENAMES",
    "XKB_CONFIG_ROOT",
    "YUNA_DBUS_DAEMON",
];

/// Builds `side` from `source` in `folder`: its tree, its target folder, its programs and its
/// log. A revision is checked out from `repository`; `canary`, for the head, is built in.
pub fn build(
    side: &str,
    source: &Source,
    repository: &Path,
    folder: &Path,
    canary: Option<Canary>,
) -> Built {
    let mut info = SideInfo {
        side: side.to_owned(),
        canary: canary.map(|c| c.name().to_owned()),
        ..SideInfo::default()
    };
    let mut built = Built {
        info: SideInfo::default(),
        programs: BTreeMap::new(),
        cases: BTreeMap::new(),
        thresholds: None,
        run_environment: Vec::new(),
    };
    if let Err(error) = build_into(source, repository, folder, canary, &mut info, &mut built) {
        info.error = Some(error);
    }
    built.info = info;
    built
}

fn build_into(
    source: &Source,
    repository: &Path,
    folder: &Path,
    canary: Option<Canary>,
    info: &mut SideInfo,
    built: &mut Built,
) -> Result<(), String> {
    fs::create_dir_all(folder).map_err(|e| format!("{}: {e}", folder.display()))?;
    let tree = match source {
        Source::Revision(revision) => {
            info.asked = revision.clone();
            let commit = commit_of(repository, revision)?;
            let tree = folder.join("tree");
            if tree.exists() && !tree_is_at(&tree, &commit) {
                // Left by an earlier run at another revision, or changed since: never reused.
                let _ = git(
                    repository,
                    &["worktree", "remove", "--force", &tree.to_string_lossy()],
                );
                if tree.exists() {
                    fs::remove_dir_all(&tree).map_err(|e| format!("{}: {e}", tree.display()))?;
                }
                git(repository, &["worktree", "prune"])?;
            }
            if !tree.exists() {
                git(
                    repository,
                    &[
                        "worktree",
                        "add",
                        "--detach",
                        &tree.to_string_lossy(),
                        &commit,
                    ],
                )?;
            }
            info.commit = Some(commit);
            info.dirty = Some(false);
            tree
        }
        Source::Path(path) => {
            info.asked = path.display().to_string();
            let tree = fs::canonicalize(path).map_err(|e| format!("{}: {e}", path.display()))?;
            info.commit = git(&tree, &["rev-parse", "HEAD"]).ok();
            info.dirty = git(&tree, &["status", "--porcelain"])
                .ok()
                .map(|changes| !changes.is_empty());
            tree
        }
    };
    built.thresholds = read_file(&tree, super::thresholds::PATH, super::thresholds::parse);
    let shell = shell_environment(&tree)?;
    let variable = |name: &str| {
        shell
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.clone())
    };
    info.mesa = variable("__EGL_VENDOR_LIBRARY_FILENAMES")
        .as_deref()
        .and_then(procfs::store_folder)
        .map(str::to_owned);
    built.run_environment = RUN_VARIABLES
        .iter()
        .filter_map(|name| Some(((*name).to_owned(), variable(name)?)))
        .collect();
    let target = folder.join("target");
    let temporary = folder.join("tmp");
    fs::create_dir_all(&temporary).map_err(|e| format!("{}: {e}", temporary.display()))?;
    let log_path = folder.join("build.log");
    let cargo = |arguments: &[&str]| -> Result<String, String> {
        let log = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
            .map_err(|e| format!("{}: {e}", log_path.display()))?;
        let errors = log.try_clone().map_err(|e| format!("build.log: {e}"))?;
        let output = Command::new(program_on(&shell, "cargo")?)
            .args(arguments)
            .current_dir(&tree)
            .env_clear()
            .envs(shell.iter().map(|(k, v)| (k, v)))
            .env("CARGO_TARGET_DIR", &target)
            .env("CARGO_PROFILE_RELEASE_DEBUG", "line-tables-only")
            .env(
                "RUSTFLAGS",
                format!(
                    "--remap-path-prefix={}=/yuna --remap-path-prefix={}=/target",
                    tree.display(),
                    target.display()
                ),
            )
            .env("TMPDIR", &temporary)
            .stdout(log)
            .stderr(errors)
            .status()
            .map_err(|e| format!("cargo: {e}"))?;
        if output.success() {
            Ok(String::new())
        } else {
            Err(format!(
                "cargo {} failed ({output}): {} says why",
                arguments.join(" "),
                log_path.display()
            ))
        }
    };
    let rustc = Command::new(program_on(&shell, "rustc")?)
        .arg("-V")
        .env_clear()
        .envs(shell.iter().map(|(k, v)| (k, v)))
        .stderr(Stdio::null())
        .output()
        .map_err(|e| format!("rustc: {e}"))?;
    info.rustc = Some(String::from_utf8_lossy(&rustc.stdout).trim().to_owned());

    let programs = folder.join("bin");
    fs::create_dir_all(&programs).map_err(|e| format!("{}: {e}", programs.display()))?;
    let mut features = vec!["rustix/use-libc-auxv"];
    if canary.is_some_and(|c| c.package() == "yuna") {
        features.push("parity-canary");
    }
    cargo(&[
        "build",
        "--release",
        "--locked",
        "-p",
        "yuna",
        "--bin",
        "yuna",
        "--features",
        &features.join(","),
    ])?;
    let mut copy = |name: &str, from: PathBuf| -> Result<(), String> {
        let to = programs.join(name);
        fs::copy(&from, &to).map_err(|e| format!("{}: {e}", from.display()))?;
        let digest = digest(&to).map_err(|e| format!("{}: {e}", to.display()))?;
        info.programs.insert(name.to_owned(), digest);
        let code = text_digest(&to).map_err(|e| format!("{}: {e}", to.display()))?;
        info.code.insert(name.to_owned(), code);
        built.programs.insert(name.to_owned(), to);
        Ok(())
    };
    copy("yuna", target.join("release/yuna"))?;
    for example in EXAMPLES {
        if !tree.join(example.source).exists() {
            continue;
        }
        let mut arguments = vec![
            "build",
            "--release",
            "--locked",
            "-p",
            example.package,
            "--example",
            example.name,
        ];
        if canary.is_some_and(|c| c.package() == example.package) {
            arguments.extend(["--features", "parity-canary"]);
        }
        cargo(&arguments)?;
        copy(
            example.name,
            target.join("release/examples").join(example.name),
        )?;
    }
    for (name, path) in &built.programs {
        if name != "yuna" {
            built
                .cases
                .insert(name.clone(), super::operations::cases(path));
        }
    }
    Ok(())
}

/// The environment of the default development shell of the flake in `tree`.
fn shell_environment(tree: &Path) -> Result<Vec<(String, String)>, String> {
    let output = Command::new("nix")
        .arg("develop")
        .arg(tree)
        .args(["--no-update-lock-file", "-c", "env", "-0"])
        .current_dir(tree)
        .stderr(Stdio::piped())
        .output()
        .map_err(|e| format!("nix develop: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "nix develop {} failed: {}",
            tree.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(procfs::parse_environment(&output.stdout))
}

/// `name`'s path on the `PATH` of `environment`.
fn program_on(environment: &[(String, String)], name: &str) -> Result<PathBuf, String> {
    let path = environment
        .iter()
        .find(|(key, _)| key == "PATH")
        .map(|(_, value)| value.as_str())
        .unwrap_or_default();
    path.split(':')
        .map(|folder| Path::new(folder).join(name))
        .find(|candidate| candidate.is_file())
        .ok_or_else(|| format!("no {name} on the shell's PATH"))
}

/// Runs git in `folder`; its output, trimmed.
pub fn git(folder: &Path, arguments: &[&str]) -> Result<String, String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(folder)
        .args(arguments)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("git: {e}"))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
    } else {
        Err(format!(
            "git {} failed: {}",
            arguments.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    }
}

/// The commit `revision` names in `repository`, in full.
pub fn commit_of(repository: &Path, revision: &str) -> Result<String, String> {
    git(
        repository,
        &["rev-parse", "--verify", &format!("{revision}^{{commit}}")],
    )
}

/// Removes the worktree a revision was checked out in, if it was.
pub fn remove_tree(repository: &Path, folder: &Path) {
    let tree = folder.join("tree");
    if tree.exists() {
        let _ = git(
            repository,
            &["worktree", "remove", "--force", &tree.to_string_lossy()],
        );
    }
}

/// The file at `path` in `tree`, read by `parse`; none if the tree has none.
fn read_file<T>(
    tree: &Path,
    path: &str,
    parse: fn(&str) -> Result<T, String>,
) -> Option<Result<T, String>> {
    let text = fs::read_to_string(tree.join(path)).ok()?;
    Some(parse(&text).map_err(|e| format!("{path}: {e}")))
}

/// Whether the worktree `tree` is at `commit`, with no change.
fn tree_is_at(tree: &Path, commit: &str) -> bool {
    git(tree, &["rev-parse", "HEAD"]).is_ok_and(|at| at == commit)
        && git(tree, &["status", "--porcelain"]).is_ok_and(|changes| changes.is_empty())
}

/// FNV-1a, 64 bits, of the `.text` section of the ELF file at `path`, in hexadecimal: its code,
/// whatever paths its other sections hold.
pub fn text_digest(path: &Path) -> Result<String, String> {
    let bytes = fs::read(path).map_err(|e| e.to_string())?;
    text_section(&bytes)
        .map(|text| format!("{:016x}", fnv1a(text)))
        .ok_or_else(|| "not a 64-bit little-endian ELF file with a .text section".to_owned())
}

/// The `.text` section of a 64-bit little-endian ELF file, from its section headers.
fn text_section(elf: &[u8]) -> Option<&[u8]> {
    if elf.get(..6)? != b"\x7fELF\x02\x01" {
        return None;
    }
    let number = |at: usize, size: usize| -> Option<u64> {
        let bytes = elf.get(at..at + size)?;
        Some(
            bytes
                .iter()
                .rev()
                .fold(0, |n, byte| n << 8 | u64::from(*byte)),
        )
    };
    let headers = number(0x28, 8)? as usize;
    let header_size = number(0x3a, 2)? as usize;
    let count = number(0x3c, 2)? as usize;
    let names = number(0x3e, 2)? as usize;
    let section = |index: usize| -> Option<(u64, usize, usize)> {
        let at = headers.checked_add(index.checked_mul(header_size)?)?;
        let name = number(at, 4)?;
        let offset = number(at + 0x18, 8)? as usize;
        let size = number(at + 0x20, 8)? as usize;
        Some((name, offset, size))
    };
    let (_, names_at, names_size) = section(names)?;
    let names = elf.get(names_at..names_at.checked_add(names_size)?)?;
    (0..count).find_map(|index| {
        let (name, offset, size) = section(index)?;
        let name = names.get(name as usize..)?;
        let end = name.iter().position(|byte| *byte == 0)?;
        (&name[..end] == b".text").then(|| elf.get(offset..offset.checked_add(size)?))?
    })
}

/// FNV-1a, 64 bits, of the file at `path`, in hexadecimal: enough to tell two builds apart.
pub fn digest(path: &Path) -> std::io::Result<String> {
    let bytes = fs::read(path)?;
    Ok(format!("{:016x}", fnv1a(&bytes)))
}

/// FNV-1a, 64 bits.
pub fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canaries_are_named_and_built_in_their_package() {
        for canary in Canary::ALL {
            assert_eq!(Canary::named(canary.name()), Some(canary));
        }
        assert_eq!(Canary::named("slow"), None);
        assert_eq!(Canary::Spin.package(), "yuna");
        assert_eq!(Canary::Layout.package(), "yuna-layout");
        assert_eq!(Canary::Motion.package(), "yuna-motion");
    }

    #[test]
    fn fnv1a_is_the_published_function() {
        // The reference values of FNV-1a, 64 bits.
        assert_eq!(fnv1a(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv1a(b"a"), 0xaf63_dc4c_8601_ec8c);
        assert_eq!(fnv1a(b"foobar"), 0x85944171f73967e8);
    }

    /// A 64-bit little-endian ELF file of three sections: none, `.text` holding `code`, and the
    /// section names.
    fn elf(code: &[u8], other: &[u8]) -> Vec<u8> {
        let names = b"\0.text\0.shstrtab\0.data\0";
        let mut file = vec![0_u8; 64];
        file[..6].copy_from_slice(b"\x7fELF\x02\x01");
        let text_at = file.len();
        file.extend_from_slice(code);
        let other_at = file.len();
        file.extend_from_slice(other);
        let names_at = file.len();
        file.extend_from_slice(names);
        let headers = file.len();
        let header = |name: u32, offset: usize, size: usize| {
            let mut header = vec![0_u8; 64];
            header[..4].copy_from_slice(&name.to_le_bytes());
            header[0x18..0x20].copy_from_slice(&(offset as u64).to_le_bytes());
            header[0x20..0x28].copy_from_slice(&(size as u64).to_le_bytes());
            header
        };
        for section in [
            header(0, 0, 0),
            header(1, text_at, code.len()),
            header(17, other_at, other.len()),
            header(7, names_at, names.len()),
        ] {
            file.extend(section);
        }
        file[0x28..0x30].copy_from_slice(&(headers as u64).to_le_bytes());
        file[0x3a..0x3c].copy_from_slice(&64_u16.to_le_bytes());
        file[0x3c..0x3e].copy_from_slice(&4_u16.to_le_bytes());
        file[0x3e..0x40].copy_from_slice(&3_u16.to_le_bytes());
        file
    }

    #[test]
    fn the_code_of_two_builds_is_told_apart_from_their_paths() {
        let one = elf(b"code", b"/w/base/tree/outputs/out/lib");
        let two = elf(b"code", b"/w/head/tree/outputs/out/lib");
        assert_eq!(text_section(&one), Some(&b"code"[..]));
        assert_eq!(text_section(&one), text_section(&two));
        assert_ne!(fnv1a(&one), fnv1a(&two));
        assert_ne!(text_section(&elf(b"edoc", b"")), Some(&b"code"[..]));
        assert_eq!(text_section(b"\x7fELF\x01\x01"), None);
        assert_eq!(text_section(&one[..70]), None);
    }

    #[test]
    fn a_program_is_found_on_the_shells_path_alone() {
        let folder = crate::testing::folder("parity-program-on");
        fs::write(folder.join("cargo"), "").unwrap();
        let environment = vec![("PATH".to_owned(), format!("/nowhere:{}", folder.display()))];
        assert_eq!(
            program_on(&environment, "cargo").unwrap(),
            folder.join("cargo")
        );
        assert!(program_on(&environment, "rustc").is_err());
        assert!(program_on(&[], "cargo").is_err());
    }
}
