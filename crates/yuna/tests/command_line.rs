//! The program run as a user runs it: its output and its exit status.

use std::process::{Command, Output};

fn run_yuna(arguments: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_yuna"))
        .args(arguments)
        .output()
        .expect("the yuna binary runs")
}

#[test]
fn version_prints_the_crate_version() {
    let output = run_yuna(&["--version"]);
    assert!(output.status.success());
    let expected = format!("yuna {}\n", env!("CARGO_PKG_VERSION"));
    assert_eq!(String::from_utf8_lossy(&output.stdout), expected);
}

#[test]
fn help_succeeds_and_names_the_options() {
    let output = run_yuna(&["--help"]);
    assert!(output.status.success());
    let help = String::from_utf8_lossy(&output.stdout);
    assert!(help.contains("--help") && help.contains("--version"));
}

#[test]
fn a_session_fails_while_no_backend_exists() {
    let output = run_yuna(&[]);
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("no backend"));
}

#[test]
fn an_unknown_argument_is_a_usage_error() {
    let output = run_yuna(&["--start"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("unknown argument --start"));
}
