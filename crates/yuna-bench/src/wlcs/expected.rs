//! The file of expected outcomes, `crates/yuna-wlcs/expected.txt`: what each WLCS test is
//! expected to give against Yuna. [`parse`] reads it, refusing any line out of its format;
//! [`write()`] writes it, which `wlcs record --merge` does. The format is in the parent module's
//! documentation.

use std::collections::BTreeMap;

use super::output::Seen;

/// The file's format, its first line `format 1`; raised when a line changes its meaning.
pub const FORMAT: u32 = 1;

/// What a test is expected to give.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Expectation {
    /// It passes.
    Pass,
    /// It fails.
    Fail,
    /// WLCS skips it, as it does a test of an extension the compositor does not offer.
    Skip,
    /// It calls a hook that Yuna's integration module cannot honour, such as touch.
    Unsupported,
    /// Its outcome varies from run to run: it is run and reported, never judged. The reason says
    /// how it varied, or why.
    Flaky(String),
}

impl Expectation {
    /// Its name in the file.
    pub fn name(&self) -> &'static str {
        match self {
            Expectation::Pass => "pass",
            Expectation::Fail => "fail",
            Expectation::Skip => "skip",
            Expectation::Unsupported => "unsupported",
            Expectation::Flaky(_) => "flaky",
        }
    }

    /// Whether a run that gave `seen` gave what was expected; a flaky test's never does.
    pub fn is(&self, seen: Seen) -> bool {
        matches!(
            (self, seen),
            (Expectation::Pass, Seen::Pass)
                | (Expectation::Fail, Seen::Fail)
                | (Expectation::Skip, Seen::Skip)
                | (Expectation::Unsupported, Seen::Unsupported)
        )
    }
}

/// The file, read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExpectedFile {
    /// The version of WLCS it was recorded with: its `wlcs <version>` line.
    pub wlcs: String,
    /// Each test, by its full gtest name, with what it is expected to give.
    pub tests: BTreeMap<String, Expectation>,
}

/// The comment at the top of the file [`write()`] writes.
const HEADER: &str = "\
# What each WLCS test is expected to give against Yuna. `yuna-bench wlcs check` runs WLCS and
# judges every test against this file; crates/yuna-bench/src/wlcs/mod.rs gives the rules and
# this format. Written by `yuna-bench wlcs record --merge`: record it again rather than edit it,
# except to give a flaky test a better reason.
";

/// Whether `text` is one word: not empty, without a space or a `#`.
fn is_word(text: &str) -> bool {
    !text.is_empty() && !text.contains(|c: char| c.is_whitespace() || c == '#')
}

/// Reads the file's `text`. Refuses, naming the line, a first line other than `format 1`, a
/// second other than `wlcs <version>`, an unknown outcome, a flaky line without its reason or
/// another with one, and a test named twice or out of order.
pub fn parse(text: &str) -> Result<ExpectedFile, String> {
    let mut lines = text
        .lines()
        .enumerate()
        .map(|(index, line)| (index + 1, line))
        .filter(|(_, line)| !line.is_empty() && !line.starts_with('#'));
    let (number, line) = lines
        .next()
        .ok_or("the file has no line but comments: its first is `format 1`")?;
    match line.strip_prefix("format ") {
        Some(format) if format == FORMAT.to_string() => {}
        Some(format) => {
            return Err(format!(
                "line {number}: format {format:?} is not one this program reads (format {FORMAT})"
            ));
        }
        None => {
            return Err(format!(
                "line {number}: the first line is `format {FORMAT}`, not {line:?}"
            ));
        }
    }
    let (number, line) = lines
        .next()
        .ok_or("the file has no `wlcs <version>` line")?;
    let wlcs = line
        .strip_prefix("wlcs ")
        .filter(|version| is_word(version))
        .ok_or_else(|| format!("line {number}: the second line is `wlcs <version>`, not {line:?}"))?
        .to_owned();
    let mut tests = BTreeMap::new();
    let mut previous: Option<&str> = None;
    for (number, line) in lines {
        let (outcome, rest) = line
            .split_once(' ')
            .ok_or_else(|| format!("line {number}: not `<outcome> <test>`: {line:?}"))?;
        let (name, reason) = match rest.split_once(" # ") {
            Some((name, reason)) => (name, Some(reason)),
            None => (rest, None),
        };
        if !is_word(name) {
            return Err(format!(
                "line {number}: a test's name is one word, without a space or a #: {line:?}"
            ));
        }
        let expectation = match (outcome, reason) {
            ("pass", None) => Expectation::Pass,
            ("fail", None) => Expectation::Fail,
            ("skip", None) => Expectation::Skip,
            ("unsupported", None) => Expectation::Unsupported,
            ("flaky", Some(reason)) if !reason.trim().is_empty() => {
                Expectation::Flaky(reason.to_owned())
            }
            ("flaky", _) => {
                return Err(format!(
                    "line {number}: a flaky test gives its reason, `flaky <test> # <reason>`"
                ));
            }
            ("pass" | "fail" | "skip" | "unsupported", Some(_)) => {
                return Err(format!(
                    "line {number}: only a flaky test's line carries a reason"
                ));
            }
            _ => {
                return Err(format!(
                    "line {number}: no outcome {outcome:?}: pass, fail, skip, unsupported or flaky"
                ));
            }
        };
        if let Some(previous) = previous {
            if name == previous {
                return Err(format!("line {number}: {name} is named twice"));
            }
            if name < previous {
                return Err(format!(
                    "line {number}: {name} comes after {previous}: the tests are sorted by name"
                ));
            }
        }
        previous = Some(name);
        tests.insert(name.to_owned(), expectation);
    }
    Ok(ExpectedFile { wlcs, tests })
}

/// The file's text for `file`: the header's comment, `format 1`, `wlcs <version>`, then one line
/// per test, sorted by name.
pub fn write(file: &ExpectedFile) -> String {
    let mut text = format!("{HEADER}format {FORMAT}\nwlcs {}\n", file.wlcs);
    for (name, expectation) in &file.tests {
        text.push_str(expectation.name());
        text.push(' ');
        text.push_str(name);
        if let Expectation::Flaky(reason) = expectation {
            text.push_str(" # ");
            text.push_str(reason);
        }
        text.push('\n');
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    const FILE: &str = "\
# A comment, then the format.
format 1
wlcs 1.8.1

# Comments and empty lines may come anywhere.
pass SelfTest.when_creating_second_client_nothing_bad_happens
flaky XdgShellStableSubsurfaces/SubsurfaceTest.place_above_simple/0 # varied: pass 4, fail 1
unsupported XdgToplevelStableTest.touch_can_not_steal_pointer_based_move
fail XdgToplevelStableTest.wm_capabilities_are_sent
skip XdgToplevelV6Test.supports_xdg_shell_v6_protocol
";

    #[test]
    fn a_file_is_read_with_each_outcome() {
        let file = parse(FILE).unwrap();
        assert_eq!(file.wlcs, "1.8.1");
        assert_eq!(file.tests.len(), 5);
        assert_eq!(
            file.tests["SelfTest.when_creating_second_client_nothing_bad_happens"],
            Expectation::Pass
        );
        assert_eq!(
            file.tests["XdgShellStableSubsurfaces/SubsurfaceTest.place_above_simple/0"],
            Expectation::Flaky("varied: pass 4, fail 1".to_owned())
        );
        assert_eq!(
            file.tests["XdgToplevelStableTest.touch_can_not_steal_pointer_based_move"],
            Expectation::Unsupported
        );
        assert_eq!(
            file.tests["XdgToplevelStableTest.wm_capabilities_are_sent"],
            Expectation::Fail
        );
        assert_eq!(
            file.tests["XdgToplevelV6Test.supports_xdg_shell_v6_protocol"],
            Expectation::Skip
        );
    }

    #[test]
    fn a_written_file_reads_back_the_same_and_writes_the_same_text() {
        let file = parse(FILE).unwrap();
        let text = write(&file);
        assert!(text.starts_with("# What each WLCS test"), "{text}");
        assert!(
            text.contains("\nformat 1\nwlcs 1.8.1\npass SelfTest."),
            "{text}"
        );
        assert!(
            text.contains(
                "\nflaky XdgShellStableSubsurfaces/SubsurfaceTest.place_above_simple/0 # varied: \
                 pass 4, fail 1\n"
            ),
            "{text}"
        );
        assert_eq!(parse(&text).unwrap(), file);
        assert_eq!(write(&parse(&text).unwrap()), text);
        // No test at all is a file too.
        let empty = ExpectedFile {
            wlcs: "1.8.1".to_owned(),
            tests: BTreeMap::new(),
        };
        assert_eq!(parse(&write(&empty)).unwrap(), empty);
    }

    #[test]
    fn a_bad_format_or_version_line_is_refused() {
        let refused = |text: &str, expected: &str| {
            let error = parse(text).unwrap_err();
            assert!(error.contains(expected), "{error:?} lacks {expected:?}");
        };
        refused("", "first is `format 1`");
        refused("# only a comment\n", "first is `format 1`");
        refused(
            "format 2\nwlcs 1.8.1\n",
            "format \"2\" is not one this program reads",
        );
        refused(
            "wlcs 1.8.1\nformat 1\n",
            "line 1: the first line is `format 1`",
        );
        refused("format 1\n", "no `wlcs <version>` line");
        refused(
            "format 1\nwlcs\n",
            "line 2: the second line is `wlcs <version>`",
        );
        refused("format 1\nwlcs \n", "line 2: the second line");
        refused("format 1\nwlcs 1.8 .1\n", "line 2: the second line");
        refused("format 1\nversion 1.8.1\n", "line 2: the second line");
    }

    #[test]
    fn duplicates_and_unsorted_lines_are_refused() {
        let error = parse("format 1\nwlcs 1.8.1\npass A.a\nfail A.a\n").unwrap_err();
        assert_eq!(error, "line 4: A.a is named twice");
        let error = parse("format 1\nwlcs 1.8.1\npass B.a\n# between\npass A.b\n").unwrap_err();
        assert_eq!(
            error,
            "line 5: A.b comes after B.a: the tests are sorted by name"
        );
    }

    #[test]
    fn a_bad_test_line_is_refused() {
        let refused = |line: &str, expected: &str| {
            let error = parse(&format!("format 1\nwlcs 1.8.1\n{line}\n")).unwrap_err();
            assert!(error.starts_with("line 3: "), "{error:?}");
            assert!(error.contains(expected), "{error:?} lacks {expected:?}");
        };
        refused("pass", "not `<outcome> <test>`");
        refused("passes A.a", "no outcome \"passes\"");
        refused("Pass A.a", "no outcome \"Pass\"");
        refused("pass A.a B.b", "one word");
        refused("pass  A.a", "one word");
        refused("pass A.a ", "one word");
        refused("flaky A.a", "gives its reason");
        refused("flaky A.a # ", "gives its reason");
        refused(
            "fail A.a # a reason",
            "only a flaky test's line carries a reason",
        );
    }
}
