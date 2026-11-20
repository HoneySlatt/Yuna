//! The figures' classes and thresholds, `crates/yuna-bench/parity-thresholds.toml`: how each figure
//! is judged, how far a parity figure may grow before it is worse, and where each came from
//! ([ADR-0142](../../../../docs/adr/0142-parity-a-pinned-baseline-figures-in-classes-three-verdicts.md)).
//!
//! They are data, tracked beside the baseline, so that loosening one is a change a reader sees:
//! the script reads the base's file and the head's, judges with the head's, and makes a figure
//! void when the head changes its class, or loosens its threshold or its budget, without a new
//! `[[move]]` of the baseline naming it ([`loosened`]).
//!
//! ```toml
//! format = 2
//!
//! [[threshold]]
//! figure = "instructions a frame, yuna"   # a kind of figure, as `figures.rs` names it
//! group = "*"                            # every scenario; or a scenario, or an operation's case
//! class = "parity"                       # parity, budget or recorded (`rule.rs`)
//! relative = 0.006                       # a share of the base's value
//! absolute = 600                         # a floor, in the figure's unit
//! basis = "3 × 0.20 %, idle's spread"   # where it came from
//! ```
//!
//! `relative` and `absolute` are a *parity* figure's alone. A *budget* figure gives `budget`, its
//! limit in its unit, in their place; a *recorded* figure gives neither. A figure takes its
//! group's entry, else its kind's `*`. Every kind has a `*`.
//!
//! Format 1, the file of M2.1's first parity script, had no `class`: the script held the classes,
//! every kind parity but `Pss_File`, recorded. A file of format 1, a base's from before format 2,
//! is read with those classes.

use super::rule::{Class, Threshold};

/// The file's path, from the workspace's root.
pub const PATH: &str = "crates/yuna-bench/parity-thresholds.toml";

/// The format of the file; raised when a key changes its meaning. Format 1 is read too.
pub const FORMAT: u64 = 2;

/// The one kind format 1 left recorded; every other kind was parity.
const RECORDED_IN_FORMAT_1: &str = "Pss_File";

/// One class and threshold.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    /// The kind of figure.
    pub figure: String,
    /// Its scenario or case, or `*`.
    pub group: String,
    /// How it is judged.
    pub class: Class,
    /// A parity figure's threshold; zero for the other classes.
    pub threshold: Threshold,
    /// Where it came from.
    pub basis: String,
}

/// The thresholds file, read.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Thresholds {
    /// Each entry, in the file's order.
    pub entries: Vec<Entry>,
}

impl Thresholds {
    /// The entry of a figure of kind `figure` in `group`: its group's, else its kind's `*`.
    pub fn lookup(&self, figure: &str, group: &str) -> Option<&Entry> {
        let find = |group: &str| {
            self.entries
                .iter()
                .find(|entry| entry.figure == figure && entry.group == group)
        };
        find(group).or_else(|| find("*"))
    }

    /// Refuses a file without a `*` for each of `figures`, or with an entry for a kind not
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

/// Reads the thresholds file's `text`, of format 1 or [`FORMAT`].
pub fn parse(text: &str) -> Result<Thresholds, String> {
    let tables = super::toml::parse(text, &["threshold"])?;
    tables[0].only(&["format"])?;
    let format = match tables[0].number("format")? {
        format if format == FORMAT as f64 => FORMAT,
        1.0 => 1,
        other => {
            return Err(format!(
                "format {other}: this script reads formats 1 and {FORMAT} only"
            ));
        }
    };
    let mut thresholds = Thresholds::default();
    for table in &tables[1..] {
        let entry = read_entry(table, format)
            .map_err(|e| format!("the threshold at line {}: {e}", table.line))?;
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

/// One `[[threshold]]` of a file of `format`.
fn read_entry(table: &super::toml::Table, format: u64) -> Result<Entry, String> {
    let figure = table.text("figure")?;
    let class = if format == 1 {
        table.only(&["figure", "group", "relative", "absolute", "basis"])?;
        if figure == RECORDED_IN_FORMAT_1 {
            Class::Recorded
        } else {
            Class::Parity
        }
    } else {
        match table.text("class")?.as_str() {
            "parity" => {
                table.only(&["figure", "group", "class", "relative", "absolute", "basis"])?;
                Class::Parity
            }
            "budget" => {
                table.only(&["figure", "group", "class", "budget", "basis"])?;
                Class::Budget(not_negative(table.number("budget")?)?)
            }
            "recorded" => {
                table.only(&["figure", "group", "class", "basis"])?;
                Class::Recorded
            }
            other => return Err(format!("no class {other:?}: parity, budget or recorded")),
        }
    };
    // Format 1 gave every kind a threshold, its recorded one included.
    let threshold = if format == 1 || class == Class::Parity {
        Threshold {
            relative: not_negative(table.number("relative")?)?,
            absolute: not_negative(table.number("absolute")?)?,
        }
    } else {
        Threshold {
            relative: 0.0,
            absolute: 0.0,
        }
    };
    Ok(Entry {
        figure,
        group: table.text("group")?,
        class,
        threshold,
        basis: table.text("basis")?,
    })
}

/// `number`, refused below 0.
fn not_negative(number: f64) -> Result<f64, String> {
    if number < 0.0 {
        Err(format!("{number}, below 0"))
    } else {
        Ok(number)
    }
}

/// Why the head's entry of a figure of kind `figure` in `group`, named `name` in the report,
/// judges it more loosely than the base's, unless one of `moves`' lists of figures names it, as
/// the report does or by its kind alone, for every group: another class, a looser threshold, a
/// higher budget. None when it is not looser, or is allowed.
pub fn loosened(
    base: &Thresholds,
    head: &Thresholds,
    figure: &str,
    group: &str,
    name: &str,
    moves: &[&[String]],
) -> Option<String> {
    let (was, now) = (base.lookup(figure, group)?, head.lookup(figure, group)?);
    let named = moves
        .iter()
        .any(|figures| figures.iter().any(|f| f == name || f == figure));
    if named {
        return None;
    }
    let unnamed = "without a [[move]] of the baseline naming it";
    match (was.class, now.class) {
        (Class::Parity, Class::Parity) => {
            let (was, now) = (was.threshold, now.threshold);
            (now.relative > was.relative || now.absolute > was.absolute).then(|| {
                format!(
                    "its threshold loosened from {} %, at least {}, to {} %, at least {}, \
                     {unnamed}",
                    was.relative * 100.0,
                    was.absolute,
                    now.relative * 100.0,
                    now.absolute
                )
            })
        }
        (Class::Budget(was), Class::Budget(now)) => {
            (now > was).then(|| format!("its budget rose from {was} to {now}, {unnamed}"))
        }
        (Class::Recorded, Class::Recorded) => None,
        (was, now) => Some(format!(
            "its class changed from {} to {}, {unnamed}",
            was.name(),
            now.name()
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FILE: &str = "format = 2\n\n\
        [[threshold]]\nfigure = \"instructions a frame, yuna\"\ngroup = \"*\"\n\
        class = \"parity\"\nrelative = 0.006\nabsolute = 600\nbasis = \"idle's spread\"\n\n\
        [[threshold]]\nfigure = \"instructions a frame, yuna\"\ngroup = \"motion\"\n\
        class = \"parity\"\nrelative = 0.011\nabsolute = 1_300\nbasis = \"motion's spread\"\n\n\
        [[threshold]]\nfigure = \"Pss_File\"\ngroup = \"*\"\nclass = \"recorded\"\n\
        basis = \"the page cache\"\n\n\
        [[threshold]]\nfigure = \"Pss_Anon\"\ngroup = \"*\"\nclass = \"budget\"\n\
        budget = 65_536\nbasis = \"a budget\"\n";

    #[test]
    fn a_figure_takes_its_groups_entry_else_its_kinds() {
        let thresholds = parse(FILE).unwrap();
        let motion = thresholds
            .lookup("instructions a frame, yuna", "motion")
            .unwrap();
        assert_eq!(motion.threshold.absolute, 1300.0);
        assert_eq!(motion.class, Class::Parity);
        let partial = thresholds
            .lookup("instructions a frame, yuna", "partial")
            .unwrap();
        assert_eq!(partial.threshold.relative, 0.006);
        assert_eq!(
            thresholds.lookup("Pss_File", "partial").unwrap().class,
            Class::Recorded
        );
        assert_eq!(
            thresholds.lookup("Pss_Anon", "many").unwrap().class,
            Class::Budget(65_536.0)
        );
        assert!(thresholds.lookup("wakes at rest", "partial").is_none());
        let kinds = ["instructions a frame, yuna", "Pss_File", "Pss_Anon"];
        assert!(thresholds.check(&kinds).is_ok());
        assert!(
            thresholds
                .check(&[&kinds[..], &["wakes at rest"]].concat())
                .unwrap_err()
                .contains("wakes at rest")
        );
        assert!(thresholds.check(&["Pss_File", "Pss_Anon"]).is_err());
    }

    #[test]
    fn a_file_that_says_less_or_other_is_refused() {
        assert!(parse(FILE).is_ok());
        assert!(parse(&FILE.replace("format = 2", "format = 3")).is_err());
        assert!(parse(&FILE.replace("absolute = 600", "absolute = -1")).is_err());
        assert!(parse(&FILE.replace("budget = 65_536", "budget = -1")).is_err());
        assert!(parse(&FILE.replace("basis = \"idle's spread\"\n", "")).is_err());
        assert!(
            parse(&FILE.replace("group = \"motion\"", "group = \"*\""))
                .unwrap_err()
                .contains("twice")
        );
        assert!(parse(&format!("{FILE}colour = \"blue\"\n")).is_err());
        assert!(parse(&FILE.replace("[[threshold]]", "[[move]]")).is_err());
        // A class it does not know, none, or keys another class takes.
        assert!(
            parse(&FILE.replace("\"recorded\"", "\"ignored\""))
                .unwrap_err()
                .contains("no class")
        );
        assert!(parse(&FILE.replacen("class = \"parity\"\n", "", 1)).is_err());
        let recorded_threshold = FILE.replace(
            "class = \"recorded\"\n",
            "class = \"recorded\"\nrelative = 0\nabsolute = 0\n",
        );
        assert!(
            parse(&recorded_threshold)
                .unwrap_err()
                .contains("not a key")
        );
        let parity_budget = FILE.replacen("absolute = 600\n", "absolute = 600\nbudget = 1\n", 1);
        assert!(
            parse(&parity_budget)
                .unwrap_err()
                .contains("`budget` is not a key")
        );
        assert!(parse(&FILE.replace("budget = 65_536\n", "")).is_err());
    }

    #[test]
    fn a_file_of_format_1_is_read_with_its_classes() {
        // A base's file from before format 2: no `class`, every entry a threshold.
        let one = "format = 1\n\n\
            [[threshold]]\nfigure = \"Pss_Anon\"\ngroup = \"*\"\nrelative = 0\n\
            absolute = 512\nbasis = \"a step\"\n\n\
            [[threshold]]\nfigure = \"Pss_File\"\ngroup = \"*\"\nrelative = 0\nabsolute = 0\n\
            basis = \"recorded, never judged\"\n";
        let thresholds = parse(one).unwrap();
        let anon = thresholds.lookup("Pss_Anon", "idle").unwrap();
        assert_eq!(
            (anon.class, anon.threshold.absolute),
            (Class::Parity, 512.0)
        );
        assert_eq!(
            thresholds.lookup("Pss_File", "idle").unwrap().class,
            Class::Recorded
        );
        // Format 1 knew no `class`, and gave every threshold.
        let with_class = one.replacen("group = \"*\"\n", "group = \"*\"\nclass = \"parity\"\n", 1);
        assert!(parse(&with_class).unwrap_err().contains("class"));
        assert!(parse(&one.replacen("absolute = 512\n", "", 1)).is_err());
    }

    #[test]
    fn the_tracked_file_in_format_1_is_read_as_it_is() {
        // The first baseline's file is the tracked one in format 1: the same thresholds, without
        // the classes. Read, it judges every figure as the tracked file does.
        let tracked = std::fs::read_to_string(crate::scenarios::workspace().join(PATH))
            .expect("the thresholds file");
        let one = tracked
            .replace("format = 2", "format = 1")
            .replace("class = \"parity\"\n", "")
            .replace("class = \"recorded\"\n", "relative = 0\nabsolute = 0\n");
        assert!(!one.contains("class ="), "a class format 1 cannot say");
        let (one, tracked) = (parse(&one).unwrap(), parse(&tracked).unwrap());
        assert_eq!(one.entries.len(), tracked.entries.len());
        for (one, now) in one.entries.iter().zip(&tracked.entries) {
            assert_eq!(
                (&one.figure, &one.group, one.class),
                (&now.figure, &now.group, now.class)
            );
            if one.class == Class::Parity {
                assert_eq!(
                    one.threshold, now.threshold,
                    "{} in {}",
                    one.figure, one.group
                );
            }
        }
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

    #[test]
    fn a_class_changed_or_a_budget_raised_needs_a_move_naming_it() {
        let base = parse(FILE).unwrap();
        let name = "motion: instructions a frame, yuna";
        let figure = "instructions a frame, yuna";
        // Parity to recorded: the figure would never be judged again.
        let recorded = parse(&FILE.replace(
            "class = \"parity\"\nrelative = 0.011\nabsolute = 1_300\n",
            "class = \"recorded\"\n",
        ))
        .unwrap();
        let reason = loosened(&base, &recorded, figure, "motion", name, &[]).unwrap();
        assert!(reason.contains("from parity to recorded"), "{reason}");
        assert_eq!(
            loosened(
                &base,
                &recorded,
                figure,
                "motion",
                name,
                &[&[name.to_owned()]]
            ),
            None
        );
        // Any change of class needs its move, recorded to parity too: its rule changed.
        assert!(loosened(&recorded, &base, figure, "motion", name, &[]).is_some());
        // A budget raised is looser; lowered, it is not.
        let higher = parse(&FILE.replace("budget = 65_536", "budget = 70_000")).unwrap();
        let reason = loosened(&base, &higher, "Pss_Anon", "many", "many: Pss_Anon", &[]).unwrap();
        assert!(reason.contains("budget rose"), "{reason}");
        assert_eq!(
            loosened(&higher, &base, "Pss_Anon", "many", "many: Pss_Anon", &[]),
            None
        );
        // A recorded figure stays recorded: nothing to loosen.
        assert_eq!(
            loosened(&base, &base, "Pss_File", "idle", "idle: Pss_File", &[]),
            None
        );
    }
}
