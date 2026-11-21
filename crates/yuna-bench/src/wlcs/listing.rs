//! WLCS's list of its tests: the output of `<runner> <module> --gtest_list_tests`, gtest's own
//! format. Each suite is a line of its name and a dot; each of its tests follows, indented by two
//! spaces; a parameterised test's line ends with a comment, `  # GetParam() = <value>`, a typed
//! suite's with `  # TypeParam = <type>`. A full name is `<suite>.<test>`, such as
//! `XdgShellStableSubsurfaces/SubsurfaceTest.place_above_simple/0`.
//!
//! gtest lists the disabled tests too, those whose suite or name starts with `DISABLED_` (or
//! holds `/DISABLED_`), and never runs them: [`parse`] leaves them out, and the file of expected
//! outcomes never names them.

use std::collections::BTreeSet;

/// One test WLCS lists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Listed {
    /// Its suite, with its prefix when it is parameterised: `Prefix/Suite`.
    pub suite: String,
    /// Its full name, `<suite>.<test>`.
    pub name: String,
}

/// The tests WLCS lists, in its order, the disabled ones left out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Listing {
    /// Each test.
    pub tests: Vec<Listed>,
}

impl Listing {
    /// Every test's full name, in WLCS's order.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.tests.iter().map(|test| test.name.as_str())
    }

    /// Each suite with its tests' full names, in WLCS's order.
    pub fn suites(&self) -> Vec<(&str, Vec<&str>)> {
        let mut suites: Vec<(&str, Vec<&str>)> = Vec::new();
        for test in &self.tests {
            match suites.last_mut() {
                Some((suite, names)) if *suite == test.suite => names.push(&test.name),
                _ => suites.push((&test.suite, vec![&test.name])),
            }
        }
        suites
    }
}

/// Whether gtest counts a test disabled, by its filter `DISABLED_*:*/DISABLED_*` matched against
/// its suite's name and its own.
pub fn is_disabled(suite: &str, test: &str) -> bool {
    let disabled = |name: &str| name.starts_with("DISABLED_") || name.contains("/DISABLED_");
    disabled(suite) || disabled(test)
}

/// `line`'s first word, if what follows it is nothing or a comment, `  # …`.
fn before_comment(line: &str) -> Option<&str> {
    match line.split_once(' ') {
        None => Some(line),
        Some((word, rest)) if rest.starts_with(" # ") => Some(word),
        Some(_) => None,
    }
}

/// Reads `--gtest_list_tests`'s output. Refuses, naming the line, any line out of its format: the
/// runner's output and its integration module's share one pipe, and a line of the module's in the
/// middle of the list would otherwise hide a test.
pub fn parse(text: &str) -> Result<Listing, String> {
    let mut tests = Vec::new();
    let mut seen = BTreeSet::new();
    let mut suite: Option<&str> = None;
    for (index, line) in text.lines().enumerate() {
        let number = index + 1;
        if line.is_empty() {
            continue;
        }
        if let Some(indented) = line.strip_prefix("  ") {
            let test = before_comment(indented)
                .filter(|test| !test.is_empty())
                .ok_or_else(|| format!("line {number}: not a test's line: {line:?}"))?;
            let suite =
                suite.ok_or_else(|| format!("line {number}: a test before any suite: {line:?}"))?;
            if is_disabled(suite, test) {
                continue;
            }
            let name = format!("{suite}.{test}");
            if !seen.insert(name.clone()) {
                return Err(format!("line {number}: {name} is listed twice"));
            }
            tests.push(Listed {
                suite: suite.to_owned(),
                name,
            });
        } else {
            suite = Some(
                before_comment(line)
                    .and_then(|word| word.strip_suffix('.'))
                    .filter(|name| !name.is_empty() && !name.contains('.'))
                    .ok_or_else(|| {
                        format!("line {number}: neither a suite's line nor a test's: {line:?}")
                    })?,
            );
        }
    }
    if tests.is_empty() {
        return Err("WLCS listed no test".to_owned());
    }
    Ok(Listing { tests })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An excerpt of WLCS 1.8.1's own list.
    const LIST: &str = include_str!("fixtures/list.txt");

    #[test]
    fn the_list_gives_full_names_with_their_suites_the_disabled_left_out() {
        let listing = parse(LIST).unwrap();
        let names: Vec<&str> = listing.names().collect();
        assert_eq!(
            names,
            [
                "BadBufferTest.test_truncated_shm_file",
                "BadBufferTest.client_lies_about_buffer_size",
                "XdgToplevelStableConfigurationTest.window_can_maximize_itself",
                "XdgToplevelStableConfigurationTest.window_can_unmaximize_itself",
                "XdgToplevelStableConfigurationTest.window_can_fullscreen_itself",
                "XdgToplevelStableConfigurationTest.window_can_unfullscreen_itself",
                "XdgToplevelStableConfigurationTest.activated_state_follows_pointer",
                "PointerCrossingSurfaceCorner/SurfacePointerMotionTest.pointer_movement/0",
                "PointerCrossingSurfaceCorner/SurfacePointerMotionTest.pointer_movement/1",
                "PointerCrossingSurfaceCorner/SurfacePointerMotionTest.pointer_movement/2",
                "PointerCrossingSurfaceCorner/SurfacePointerMotionTest.pointer_movement/3",
                "Anchors/LayerSurfaceErrorsTest.\
                 specifying_zero_size_without_corresponding_anchors_is_an_error/0",
                "Anchors/LayerSurfaceErrorsTest.\
                 specifying_zero_size_without_corresponding_anchors_is_an_error/1",
                "WpViewporterSrcParamsTest.\
                 raises_protocol_error_on_invalid_value/src_width_must_be_non_negative",
                "WpViewporterSrcParamsTest.\
                 raises_protocol_error_on_invalid_value/src_height_must_be_non_negative",
            ]
        );
        // SecondBadBufferTest holds a disabled test alone: it is not a suite to run.
        let suites: Vec<(&str, usize)> = listing
            .suites()
            .into_iter()
            .map(|(suite, names)| (suite, names.len()))
            .collect();
        assert_eq!(
            suites,
            [
                ("BadBufferTest", 2),
                ("XdgToplevelStableConfigurationTest", 5),
                ("PointerCrossingSurfaceCorner/SurfacePointerMotionTest", 4),
                ("Anchors/LayerSurfaceErrorsTest", 2),
                ("WpViewporterSrcParamsTest", 2),
            ]
        );
    }

    #[test]
    fn disabled_tests_are_named_as_gtest_names_them() {
        assert!(is_disabled(
            "SecondBadBufferTest",
            "DISABLED_test_truncated_shm_file"
        ));
        assert!(is_disabled("DISABLED_Suite", "test"));
        assert!(is_disabled("Prefix/DISABLED_Suite", "test"));
        assert!(is_disabled("Suite", "test/DISABLED_param"));
        assert!(!is_disabled("Suite", "test_DISABLED_not"));
        assert!(!is_disabled("NotDISABLED_Suite", "test"));
    }

    #[test]
    fn a_typed_suites_comment_is_read_too() {
        let listing = parse("Typed/0.  # TypeParam = int\n  works\n").unwrap();
        assert_eq!(listing.names().collect::<Vec<_>>(), ["Typed/0.works"]);
    }

    #[test]
    fn a_line_out_of_the_format_is_refused() {
        let refused = |text: &str, expected: &str| {
            let error = parse(text).unwrap_err();
            assert!(error.contains(expected), "{error:?} lacks {expected:?}");
        };
        refused("", "listed no test");
        refused("SecondBadBufferTest.\n  DISABLED_test\n", "listed no test");
        refused("  orphan\n", "line 1: a test before any suite");
        // A line of the module's, between two of the list's.
        refused(
            "Suite.\n  a\nyuna-wlcs: unsupported: touch.\n  b\n",
            "line 3: neither a suite's line nor a test's",
        );
        refused("Suite\n  a\n", "line 1: neither");
        refused("Suite.\n  a b\n", "line 2: not a test's line");
        refused("Suite.\n   a\n", "line 2: not a test's line");
        refused("Suite.\n  a\n  a\n", "line 3: Suite.a is listed twice");
    }
}
