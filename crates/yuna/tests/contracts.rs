//! Yuna's contracts held as text
//! ([ADR-0079](../../../docs/adr/0079-yunas-contracts-in-0-x-changes-announced-never-silent.md)):
//! `yuna --help` and the MCP server's `tools/list` are compared with files kept beside this test,
//! in `tests/contracts/`, so that a change to either is an edit of its file, seen in the diff.
//!
//! A change made on purpose rewrites the files with `YUNA_WRITE_CONTRACTS=1 cargo test --test
//! contracts`, and adds its changelog fragment (`changelog.d/README.md`).

#[path = "common/folders.rs"]
mod folders;

use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use folders::fresh_folder;
use serde_json::{Value, json};

/// The folder of the files the contracts are compared with.
fn contracts_folder() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/contracts")
}

/// Compares `actual` with the file `name` of [`contracts_folder`], or writes it there when
/// `YUNA_WRITE_CONTRACTS` is set.
fn assert_held(name: &str, actual: &str) {
    let path = contracts_folder().join(name);
    if std::env::var_os("YUNA_WRITE_CONTRACTS").is_some() {
        fs::write(&path, actual).expect("the contract's file is written");
        return;
    }
    let held = fs::read_to_string(&path).unwrap_or_default();
    assert!(
        held == actual,
        "{name} differs from the contract held in {}.\n\
         If the change is meant, rewrite it with `YUNA_WRITE_CONTRACTS=1 cargo test --test \
         contracts` and add its changelog fragment (changelog.d/README.md).\n\
         --- held\n{held}\n--- now\n{actual}",
        path.display()
    );
}

#[test]
fn the_help_is_the_one_held() {
    let output = Command::new(env!("CARGO_BIN_EXE_yuna"))
        .arg("--help")
        .output()
        .expect("the yuna binary runs");
    assert!(output.status.success());
    let help = String::from_utf8(output.stdout).expect("the help is UTF-8");
    assert_held("help.txt", &help);
}

#[test]
fn the_mcp_tools_are_the_ones_held() {
    let runtime = fresh_folder("contracts");
    let mut child = Command::new(env!("CARGO_BIN_EXE_yuna"))
        .arg("mcp")
        .env("XDG_RUNTIME_DIR", &runtime)
        .env("YUNA_LOG", "warn")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("the yuna binary runs");
    let request = json!({
        "jsonrpc": "2.0", "id": 1, "method": "tools/list",
        "params": {
            "_meta": {
                "io.modelcontextprotocol/protocolVersion": yuna::mcp::MODERN_REVISION,
                "io.modelcontextprotocol/clientCapabilities": {},
            }
        }
    });
    let mut input = child.stdin.take().expect("its input");
    writeln!(input, "{request}").expect("the request is sent");
    let mut line = String::new();
    BufReader::new(child.stdout.take().expect("its output"))
        .read_line(&mut line)
        .expect("a reply");
    drop(input);
    let status = child.wait().expect("yuna stops when its input closes");
    let _ = fs::remove_dir_all(&runtime);
    assert!(status.success(), "yuna mcp failed: {status}");
    let reply: Value = serde_json::from_str(&line).expect("the reply is JSON");
    let tools = &reply["result"]["tools"];
    assert!(tools.is_array(), "no tools in {reply}");
    let text = serde_json::to_string_pretty(tools).expect("the tools as JSON") + "\n";
    assert_held("tools-list.json", &text);
}
