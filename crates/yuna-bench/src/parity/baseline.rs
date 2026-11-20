//! The pinned baseline, `crates/yuna-bench/parity-baseline.toml`
//! ([ADR-0142](../../../../docs/adr/0142-parity-a-pinned-baseline-figures-in-classes-three-verdicts.md)):
//! the commit every push is compared with, and the history of its moves.
//!
//! The file is a small part of TOML (`toml.rs`): a `format`, a `commit` and `[[move]]` tables.
//! Anything else is refused, never guessed.
//!
//! ```toml
//! format = 1
//! commit = "<40 hexadecimal digits>"
//!
//! [[move]]
//! commit = "<the same>"
//! figures = ["partial: instructions a frame, yuna"]
//! reason = "why the figures moved"
//! allowed_by = "ADR-0000, or the owner's go of <date>"
//! ```
//!
//! Each move names the commit the baseline moved to, the figures it moved, why, and the ADR or
//! the owner's go that allows it. The newest move is last, and its commit is `commit`. A commit
//! cannot name itself, so the baseline is always an earlier commit than the file that pins it,
//! and its own file never holds the move to it.
//!
//! The script reads the files of the head and of the push before ([`at`]) before it builds
//! anything, so that a run that cannot tell which moves are new fails at once.

use std::path::Path;

use super::build::{self, Source};

/// The file's path, from the workspace's root.
pub const PATH: &str = "crates/yuna-bench/parity-baseline.toml";

/// The format of the file; raised when a key changes its meaning.
pub const FORMAT: u64 = 1;

/// The baseline file, read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Baseline {
    /// The commit every push is compared with.
    pub commit: String,
    /// Its moves, the oldest first.
    pub moves: Vec<Move>,
}

/// One move of the baseline.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Move {
    /// The commit it moved to.
    pub commit: String,
    /// The figures it moved, by name; none for the first baseline.
    pub figures: Vec<String>,
    /// Why.
    pub reason: String,
    /// The ADR or the owner's go that allows it.
    pub allowed_by: String,
}

/// Reads the baseline file's `text`.
pub fn parse(text: &str) -> Result<Baseline, String> {
    let tables = super::toml::parse(text, &["move"])?;
    let top = &tables[0];
    top.only(&["format", "commit"])?;
    match top.number("format")? {
        format if format == FORMAT as f64 => {}
        other => {
            return Err(format!(
                "format {other}: this script reads format {FORMAT} only"
            ));
        }
    }
    let commit = top.text("commit")?;
    check_commit(&commit)?;
    let moves = tables[1..]
        .iter()
        .map(|table| {
            let read = || -> Result<Move, String> {
                table.only(&["commit", "figures", "reason", "allowed_by"])?;
                let commit = table.text("commit")?;
                check_commit(&commit)?;
                let reason = table.text("reason")?;
                let allowed_by = table.text("allowed_by")?;
                if reason.trim().is_empty() || allowed_by.trim().is_empty() {
                    return Err("an empty `reason` or `allowed_by`".to_owned());
                }
                Ok(Move {
                    commit,
                    figures: table.list("figures")?,
                    reason,
                    allowed_by,
                })
            };
            read().map_err(|e| format!("the move at line {}: {e}", table.line))
        })
        .collect::<Result<Vec<Move>, String>>()?;
    match moves.last() {
        None => return Err("no `[[move]]`: the first baseline is a move too".to_owned()),
        Some(last) if last.commit != commit => {
            return Err(format!(
                "the newest move is to {}, but `commit` is {commit}",
                last.commit
            ));
        }
        Some(_) => {}
    }
    Ok(Baseline { commit, moves })
}

/// The baseline file of the tree `source` names, read; none if that tree has none. A revision's
/// is read from `repository`'s history, a folder's from the folder, uncommitted changes included.
pub fn at(source: &Source, repository: &Path) -> Result<Option<Baseline>, String> {
    let (text, place) = match source {
        Source::Revision(revision) => {
            let commit = build::commit_of(repository, revision)?;
            // Empty when the commit has no such file; an error only for a commit git cannot read.
            if build::git(repository, &["ls-tree", "--name-only", &commit, "--", PATH])?.is_empty()
            {
                return Ok(None);
            }
            let text = build::git(repository, &["show", &format!("{commit}:{PATH}")])?;
            (
                text,
                format!("{PATH} at {}", &commit[..commit.len().min(12)]),
            )
        }
        Source::Path(folder) => {
            let path = folder.join(PATH);
            match std::fs::read_to_string(&path) {
                Ok(text) => (text, path.display().to_string()),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
                Err(error) => return Err(format!("{}: {error}", path.display())),
            }
        }
    };
    parse(&text).map(Some).map_err(|e| format!("{place}: {e}"))
}

/// A commit is named in full, so that no later commit can share its prefix.
fn check_commit(commit: &str) -> Result<(), String> {
    if commit.len() == 40 && commit.bytes().all(|b| b.is_ascii_hexdigit()) {
        Ok(())
    } else {
        Err(format!(
            "{commit:?} is not a commit's 40 hexadecimal digits"
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: &str = "2e14b86aafd7df6d76fc18c79382d9a0ce1d6ecf";
    const B: &str = "1877694aafd7df6d76fc18c79382d9a0ce1d6eca";

    fn file(body: &str) -> String {
        format!("# The baseline.\nformat = 1\ncommit = \"{B}\"\n\n{body}")
    }

    #[test]
    fn the_file_gives_its_commit_and_its_moves() {
        let text = file(&format!(
            "[[move]]\ncommit = \"{A}\"\nfigures = []\nreason = \"the first baseline\"\n\
             allowed_by = \"ADR-0142\"\n\n[[move]]  \n  commit = \"{B}\" # moved\n\
             figures = [\"partial: allocations a frame\", \"many: Pss_Anon\"]\n\
             reason = \"Vulkan, \\\"no GLES\\\"\"\nallowed_by = \"the owner's go, 2026-12-01\"\n"
        ));
        let baseline = parse(&text).unwrap();
        assert_eq!(baseline.commit, B);
        assert_eq!(baseline.moves.len(), 2);
        assert_eq!(baseline.moves[0].figures, Vec::<String>::new());
        assert_eq!(
            baseline.moves[1].figures,
            ["partial: allocations a frame", "many: Pss_Anon"]
        );
        assert_eq!(baseline.moves[1].reason, "Vulkan, \"no GLES\"");
    }

    #[test]
    fn a_file_that_says_less_or_other_is_refused() {
        let first = format!(
            "[[move]]\ncommit = \"{B}\"\nfigures = []\nreason = \"r\"\nallowed_by = \"a\"\n"
        );
        assert!(parse(&file(&first)).is_ok());
        // The newest move is not to `commit`.
        let behind = first.replace(B, A);
        assert!(parse(&file(&behind)).unwrap_err().contains("newest move"));
        // No move at all, a move without its reason, a key unknown or given twice.
        assert!(parse(&file("")).unwrap_err().contains("[[move]]"));
        let no_reason = first.replace("reason = \"r\"\n", "");
        assert!(parse(&file(&no_reason)).unwrap_err().contains("reason"));
        let unknown = format!("{first}colour = \"blue\"\n");
        assert!(parse(&file(&unknown)).unwrap_err().contains("colour"));
        let twice = format!("{first}reason = \"again\"\n");
        assert!(parse(&file(&twice)).unwrap_err().contains("twice"));
        // A short commit, which a later one could share; a later format; none.
        assert!(parse(&file(&first).replace(B, "2e14b86")).is_err());
        assert!(
            parse(&file(&first).replace("format = 1", "format = 2"))
                .unwrap_err()
                .contains("format 2")
        );
        assert!(parse(&file(&first).replace("format = 1\n", "")).is_err());
        // Malformed values.
        let open_list = first.replace("figures = []", "figures = [\"a\"");
        assert!(parse(&file(&open_list)).is_err());
        let open_string = first.replace("reason = \"r\"", "reason = \"r");
        assert!(parse(&file(&open_string)).is_err());
        let no_comma = first.replace("figures = []", "figures = [\"a\" \"b\"]");
        assert!(parse(&file(&no_comma)).is_err());
        assert!(parse("").is_err());
    }

    #[test]
    fn a_trees_file_is_read_from_its_folder_or_its_revision() {
        let folder = crate::testing::folder("parity-baseline-at");
        let source = Source::Path(folder.clone());
        let repository = crate::scenarios::workspace();
        // A tree from before the file has none.
        assert_eq!(at(&source, &repository), Ok(None));
        let file = folder.join(PATH);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        let text = format!(
            "format = 1\ncommit = \"{A}\"\n[[move]]\ncommit = \"{A}\"\nfigures = []\n\
             reason = \"r\"\nallowed_by = \"a\"\n"
        );
        std::fs::write(&file, &text).unwrap();
        assert_eq!(at(&source, &repository).unwrap().unwrap().commit, A);
        // A file that does not read fails, naming itself; never read as absent.
        std::fs::write(&file, text.replace("format = 1", "format = 9")).unwrap();
        let error = at(&source, &repository).unwrap_err();
        assert!(
            error.contains(PATH) && error.contains("format 9"),
            "{error}"
        );
        // A revision's is its commit's, as git holds it.
        let tracked = parse(&std::fs::read_to_string(repository.join(PATH)).unwrap()).unwrap();
        let committed = at(&Source::Revision("HEAD".to_owned()), &repository)
            .expect("HEAD's file")
            .expect("HEAD has one");
        assert_eq!(committed.moves.first(), tracked.moves.first());
        assert!(
            at(
                &Source::Revision("no-such-revision".to_owned()),
                &repository
            )
            .is_err()
        );
    }

    #[test]
    fn the_tracked_file_reads() {
        let text = std::fs::read_to_string(crate::scenarios::workspace().join(PATH))
            .expect("the baseline file");
        let baseline = parse(&text).expect("the tracked baseline");
        assert_eq!(
            baseline.moves.last().map(|m| &m.commit),
            Some(&baseline.commit)
        );
    }
}
