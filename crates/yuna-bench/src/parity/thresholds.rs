//! The thresholds, `crates/yuna-bench/parity-thresholds.toml`: how far each parity figure may
//! grow before it is worse, and the spread each came from
//! ([ADR-0142](../../../../docs/adr/0142-parity-a-pinned-baseline-figures-in-classes-three-verdicts.md)).
//!
//! They are data, tracked beside the baseline, so that loosening one is a change a reader sees:
//! the script reads the base's file and the head's, judges with the head's, and makes a figure
//! void when the head loosens its threshold without a new `[[move]]` of the baseline naming it
//! ([`loosened`]).
//!
//! ```toml
//! format = 1
//!
//! [[threshold]]
//! figure = "instructions a frame, yuna"   # a kind of figure, as `figures.rs` names it
//! group = "*"                            # every scenario; or a scenario, or an operation's case
//! relative = 0.006                       # a share of the base's value
//! absolute = 600                         # a floor, in the figure's unit
//! basis = "3 × 0.20 %, idle's spread"   # where it came from
//! ```
//!
//! A figure takes its group's threshold, else its kind's `*`. Every kind has a `*`.

use super::rule::Threshold;

/// The file's path, from the workspace's root.
pub const PATH: &str = "crates/yuna-bench/parity-thresholds.toml";

/// The format of the file; raised when a key changes its meaning.
pub const FORMAT: u64 = 1;

/// One threshold.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    /// The kind of figure.
    pub figure: String,
    /// Its scenario or case, or `*`.
    pub group: String,
    /// The threshold.
    pub threshold: Threshold,
    /// Where it came from.
    pub basis: String,
}

/// The thresholds file, read.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Thresholds {
    /// Each threshold, in the file's order.
    pub entries: Vec<Entry>,
}

impl Thresholds {
    /// The threshold of a figure of kind `figure` in `group`: its group's, else its kind's `*`.
    pub fn lookup(&self, figure: &str, group: &str) -> Option<&Entry> {
        let find = |group: &str| {
            self.entries
                .iter()
                .find(|entry| entry.figure == figure && entry.group == group)
        };
        find(group).or_else(|| find("*"))
    }

    /// Refuses a file without a `*` for each of `figures`, or with a threshold for a kind not
    /// among them.
    pub fn check(&self, figures: &[&str]) -> Result<(), String> {
        if let Some(missing) = figures.iter().find(|f| self.lookup(f, "*").is_none()) {
            return Err(format!("no threshold for {missing:?} in group \"*\""));
        }
        if let Some(unknown) = self
            .entries
            .iter()
            .find(|entry| !figures.contains(&entry.figure.as_str()))
        {
            return Err(format!(
                "no figure {:?}, which a threshold names",
                unknown.figure
            ));
        }
        Ok(())
    }
}

/// Reads the thresholds file's `text`.
pub fn parse(text: &str) -> Result<Thresholds, String> {
    let tables = super::toml::parse(text, &["threshold"])?;
    tables[0].only(&["format"])?;
    match tables[0].number("format")? {
        format if format == FORMAT as f64 => {}
        other => {
            return Err(format!(
                "format {other}: this script reads format {FORMAT} only"
            ));
        }
    }
    let mut thresholds = Thresholds::default();
    for table in &tables[1..] {
        let read = || -> Result<Entry, String> {
            table.only(&["figure", "group", "relative", "absolute", "basis"])?;
            let threshold = Threshold {
                relative: table.number("relative")?,
                absolute: table.number("absolute")?,
            };
            if threshold.relative < 0.0 || threshold.absolute < 0.0 {
                return Err("a threshold below 0".to_owned());
            }
            Ok(Entry {
                figure: table.text("figure")?,
                group: table.text("group")?,
                threshold,
                basis: table.text("basis")?,
            })
        };
        let entry = read().map_err(|e| format!("the threshold at line {}: {e}", table.line))?;
        if thresholds
            .entries
            .iter()
            .any(|e| e.figure == entry.figure && e.group == entry.group)
        {
            return Err(format!(
                "the threshold at line {}: {:?} in {:?} given twice",
                table.line, entry.figure, entry.group
            ));
        }
        thresholds.entries.push(entry);
    }
    Ok(thresholds)
}

/// Why the head's threshold of a figure of kind `figure` in `group`, named `name` in the report,
/// is looser than the base's, unless one of `moves`' lists of figures names it, as the report
/// does or by its kind alone, for every group; none when it is not looser, or is allowed.
pub fn loosened(
    base: &Thresholds,
    head: &Thresholds,
    figure: &str,
    group: &str,
    name: &str,
    moves: &[&[String]],
) -> Option<String> {
    let (was, now) = (
        base.lookup(figure, group)?.threshold,
        head.lookup(figure, group)?.threshold,
    );
    let looser = now.relative > was.relative || now.absolute > was.absolute;
    let named = moves
        .iter()
        .any(|figures| figures.iter().any(|f| f == name || f == figure));
    (looser && !named).then(|| {
        format!(
            "its threshold loosened from {} %, at least {}, to {} %, at least {}, without a \
             [[move]] of the baseline naming it",
            was.relative * 100.0,
            was.absolute,
            now.relative * 100.0,
            now.absolute
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const FILE: &str = "format = 1\n\n\
        [[threshold]]\nfigure = \"instructions a frame, yuna\"\ngroup = \"*\"\n\
        relative = 0.006\nabsolute = 600\nbasis = \"idle's spread\"\n\n\
        [[threshold]]\nfigure = \"instructions a frame, yuna\"\ngroup = \"motion\"\n\
        relative = 0.011\nabsolute = 1_300\nbasis = \"motion's spread\"\n";

    #[test]
    fn a_figure_takes_its_groups_threshold_else_its_kinds() {
        let thresholds = parse(FILE).unwrap();
        let motion = thresholds
            .lookup("instructions a frame, yuna", "motion")
            .unwrap();
        assert_eq!(motion.threshold.absolute, 1300.0);
        let partial = thresholds
            .lookup("instructions a frame, yuna", "partial")
            .unwrap();
        assert_eq!(partial.threshold.relative, 0.006);
        assert!(thresholds.lookup("Pss_Anon", "partial").is_none());
        assert!(thresholds.check(&["instructions a frame, yuna"]).is_ok());
        assert!(
            thresholds
                .check(&["instructions a frame, yuna", "Pss_Anon"])
                .unwrap_err()
                .contains("Pss_Anon")
        );
        assert!(thresholds.check(&["Pss_Anon"]).is_err());
    }

    #[test]
    fn a_file_that_says_less_or_other_is_refused() {
        assert!(parse(&FILE.replace("format = 1", "format = 2")).is_err());
        assert!(parse(&FILE.replace("absolute = 600", "absolute = -1")).is_err());
        assert!(parse(&FILE.replace("basis = \"idle's spread\"\n", "")).is_err());
        assert!(
            parse(&FILE.replace("group = \"motion\"", "group = \"*\""))
                .unwrap_err()
                .contains("twice")
        );
        assert!(parse(&format!("{FILE}colour = \"blue\"\n")).is_err());
        assert!(parse(&FILE.replace("[[threshold]]", "[[move]]")).is_err());
    }

    #[test]
    fn the_tracked_file_reads_and_has_every_kind() {
        let text = std::fs::read_to_string(crate::scenarios::workspace().join(PATH))
            .expect("the thresholds file");
        let thresholds = parse(&text).expect("the tracked thresholds");
        thresholds
            .check(&crate::parity::figures::kind_names())
            .expect("a threshold for every kind");
    }

    #[test]
    fn a_threshold_loosened_needs_a_move_naming_it() {
        let base = parse(FILE).unwrap();
        let looser = parse(&FILE.replace("absolute = 1_300", "absolute = 2_000")).unwrap();
        let figure = "instructions a frame, yuna";
        let name = "motion: instructions a frame, yuna";
        let reason = loosened(&base, &looser, figure, "motion", name, &[]).unwrap();
        assert!(reason.contains("at least 1300, to"), "{reason}");
        // Another group's threshold did not move.
        assert_eq!(
            loosened(&base, &looser, figure, "partial", "partial: x", &[]),
            None
        );
        // A move that names the figure, as the report does or by its kind, allows it.
        let named = [name.to_owned()];
        assert_eq!(
            loosened(&base, &looser, figure, "motion", name, &[&named]),
            None
        );
        let kind = [figure.to_owned()];
        assert_eq!(
            loosened(&base, &looser, figure, "motion", name, &[&kind]),
            None
        );
        let other = ["partial: Pss_Anon".to_owned()];
        assert!(loosened(&base, &looser, figure, "motion", name, &[&other]).is_some());
        // Tighter is never a loosening.
        assert_eq!(loosened(&looser, &base, figure, "motion", name, &[]), None);
    }
}
