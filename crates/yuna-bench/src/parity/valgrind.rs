//! valgrind's two tools, as the parity script runs them, and the readers of what they write:
//! callgrind counts instructions, DHAT counts heap allocations
//! ([ADR-0143](../../../../docs/adr/0143-allocations-a-frame-counted-by-dhat.md)).
//!
//! Both follow every process the measured one starts (`--trace-children=yes`), but the programs
//! a seat runs for Yuna, which are not Yuna ([`SKIPPED`]), and write one file per process
//! (`%p`). Only the files of processes running the build's own `yuna` are summed: at M2.12 a seat
//! moves to a sibling Yuna, which these figures then follow. `--fair-sched=yes` runs threads in
//! turn, so that llvmpipe's threads take the same turns in two runs.
//!
//! What valgrind counts does not depend on the machine's load: it simulates the processor, one
//! thread at a time.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;

/// The programs Yuna starts that are not Yuna, never traced: a seat's accessibility bus and its
/// X server. Their costs are not Yuna's.
pub const SKIPPED: &str = "*/dbus-daemon,*/Xwayland";

/// Which of valgrind's tools.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Tool {
    /// Instructions.
    Callgrind,
    /// Heap allocations.
    Dhat,
}

impl Tool {
    /// Its name.
    pub fn name(self) -> &'static str {
        match self {
            Tool::Callgrind => "callgrind",
            Tool::Dhat => "dhat",
        }
    }
}

/// The command that runs `program` under `tool`, writing its files into `folder`.
pub fn command(valgrind: &Path, tool: Tool, folder: &Path) -> Vec<String> {
    let folder = folder.display();
    let mut arguments = vec![
        valgrind.display().to_string(),
        format!("--tool={}", tool.name()),
        "--trace-children=yes".to_owned(),
        format!("--trace-children-skip={SKIPPED}"),
        "--fair-sched=yes".to_owned(),
        // No gdbserver: nothing attaches, and nothing waits on its FIFOs in /tmp.
        "--vgdb=no".to_owned(),
        format!("--log-file={folder}/valgrind.%p.log"),
    ];
    arguments.push(match tool {
        Tool::Callgrind => format!("--callgrind-out-file={folder}/callgrind.%p"),
        Tool::Dhat => format!("--dhat-out-file={folder}/dhat.%p.json"),
    });
    arguments
}

/// What callgrind counted in one process.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Instructions {
    /// The command it ran.
    pub command: String,
    /// Every instruction.
    pub total: u64,
    /// By object: the executable or the shared library the instructions are in; `???` for code
    /// with no object, llvmpipe's compiled shaders.
    pub by_object: BTreeMap<String, u64>,
}

/// Reads a callgrind file's `text`: its command, its total and each object's own instructions.
///
/// A cost line after a `calls=` line is the callee's cost at that call: counted with its caller's
/// object it would count the callee twice, so it is left out; what is summed is each function's
/// own cost, whose sum is callgrind's `summary`, checked.
pub fn read_callgrind(text: &str) -> Result<Instructions, String> {
    let mut names: BTreeMap<String, String> = BTreeMap::new();
    let mut current = "???".to_owned();
    let mut after_call = false;
    let mut summary = None;
    let mut result = Instructions::default();
    for line in text.lines() {
        if let Some(command) = line.strip_prefix("cmd:") {
            result.command = command.trim().to_owned();
        } else if let Some(value) = line.strip_prefix("summary:") {
            summary = value.trim().parse::<u64>().ok();
        } else if let Some(spec) = line.strip_prefix("ob=") {
            current = object_name(spec, &mut names);
        } else if let Some(spec) = line.strip_prefix("cob=") {
            // A callee's object, named here once for later lines: not the current object.
            object_name(spec, &mut names);
        } else if line.starts_with("calls=") {
            after_call = true;
        } else if line.starts_with(|c: char| c.is_ascii_digit() || matches!(c, '+' | '-' | '*')) {
            let mut fields = line.split_whitespace();
            let (Some(_position), Some(cost), None) = (fields.next(), fields.next(), fields.next())
            else {
                continue;
            };
            let Ok(cost) = cost.parse::<u64>() else {
                continue;
            };
            if std::mem::take(&mut after_call) {
                continue;
            }
            result.total += cost;
            *result.by_object.entry(current.clone()).or_default() += cost;
        }
    }
    match summary {
        Some(summary) if summary == result.total => Ok(result),
        Some(summary) => Err(format!(
            "the functions' own costs sum to {}, not callgrind's summary {summary}",
            result.total
        )),
        None => Err("no summary: the process did not end".to_owned()),
    }
}

/// The object a `ob=` or `cob=` line names: `(id) name` names it, `(id)` repeats it.
fn object_name(spec: &str, names: &mut BTreeMap<String, String>) -> String {
    let spec = spec.trim();
    if let Some(rest) = spec.strip_prefix('(')
        && let Some((id, name)) = rest.split_once(')')
    {
        let name = name.trim();
        if name.is_empty() {
            return names.get(id).cloned().unwrap_or_else(|| "???".to_owned());
        }
        names.insert(id.to_owned(), name.to_owned());
        return name.to_owned();
    }
    spec.to_owned()
}

/// What DHAT counted in one process.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Allocations {
    /// The command it ran.
    pub command: String,
    /// Heap blocks allocated.
    pub blocks: u64,
    /// Bytes allocated.
    pub bytes: u64,
    /// Blocks allocated by a call that went through the measured binary's own code.
    pub own_blocks: u64,
    /// The bytes the heap still held when the process ended (DHAT's `eb`): what it kept, a leak
    /// among it, when it is ended by a signal before it frees anything. Not the heap's peak,
    /// `t-gmax`: Yuna's is at its start, LLVM compiling llvmpipe's shaders, and never moves after;
    /// and DHAT updates each site's own peak only when the whole heap peaks, so neither sees a
    /// leak below that.
    pub end_bytes: u64,
}

/// Reads a DHAT file's JSON `text`. A call went through `binary`'s own code when a frame of its
/// stack is in it: the frame names it, or names a Rust source file, which only the build's own
/// binary, compiled with line tables, gives here (Mesa and glibc are stripped).
pub fn read_dhat(text: &str, binary: &Path) -> Result<Allocations, String> {
    let data: Value = serde_json::from_str(text).map_err(|e| format!("DHAT's file: {e}"))?;
    let frames: Vec<&str> = data
        .get("ftbl")
        .and_then(Value::as_array)
        .ok_or("DHAT's file has no frame table")?
        .iter()
        .map(|frame| frame.as_str().unwrap_or_default())
        .collect();
    let in_binary = format!("(in {})", binary.display());
    let own: Vec<bool> = frames
        .iter()
        .map(|frame| frame.ends_with(&in_binary) || is_rust_source(frame))
        .collect();
    let mut result = Allocations {
        command: data
            .get("cmd")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        ..Allocations::default()
    };
    let points = data
        .get("pps")
        .and_then(Value::as_array)
        .ok_or("DHAT's file has no program points")?;
    let number = |point: &Value, key: &str| point.get(key).and_then(Value::as_u64).unwrap_or(0);
    for point in points {
        let blocks = number(point, "tbk");
        result.blocks += blocks;
        result.bytes += number(point, "tb");
        result.end_bytes += number(point, "eb");
        let through_own = point
            .get("fs")
            .and_then(Value::as_array)
            .is_some_and(|stack| {
                stack.iter().any(|index| {
                    index
                        .as_u64()
                        .and_then(|i| own.get(i as usize))
                        .copied()
                        .unwrap_or(false)
                })
            });
        if through_own {
            result.own_blocks += blocks;
        }
    }
    Ok(result)
}

/// Whether a DHAT frame such as `0x4A: f (render.rs:186)` names a Rust source file.
fn is_rust_source(frame: &str) -> bool {
    let Some(location) = frame
        .strip_suffix(')')
        .and_then(|rest| rest.rsplit_once('('))
        .map(|(_, location)| location)
    else {
        return false;
    };
    location
        .rsplit_once(':')
        .is_some_and(|(file, line)| file.ends_with(".rs") && line.parse::<u32>().is_ok())
}

/// The files `tool` wrote into `folder`, one per process.
pub fn files(folder: &Path, tool: Tool) -> Vec<PathBuf> {
    let prefix = match tool {
        Tool::Callgrind => "callgrind.",
        Tool::Dhat => "dhat.",
    };
    let mut files: Vec<PathBuf> = fs::read_dir(folder)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with(prefix) && !name.ends_with(".log"))
        })
        .collect();
    files.sort();
    files
}

/// Whether a process's `command`, its program then its arguments as valgrind writes them, ran
/// `binary`: the whole path, which may hold a space, then nothing or an argument. valgrind does
/// not quote, so another program at `binary`'s path followed by a space and more would read the
/// same; none can be: `binary` is a side's copy, `<work>/<side>/bin/<name>`, and nothing in
/// `bin` is a folder whose name starts with `<name> `.
pub fn runs(command: &str, binary: &Path) -> bool {
    command
        .strip_prefix(&*binary.to_string_lossy())
        .is_some_and(|rest| rest.is_empty() || rest.starts_with(' '))
}

#[cfg(test)]
mod tests {
    use super::*;

    const CALLGRIND: &str = "# callgrind format
version: 1
creator: callgrind-3.27.1
pid: 7
cmd:  /w/bin/base/yuna mcp --manual-clock
part: 1

positions: line
events: Ir
summary: 1175

ob=(1) /nix/store/x-mesa/lib/libgallium.so
fl=(1) ???
fn=(1) lp_rast
0 1000
cob=(2) /w/bin/base/yuna
cfi=(2) ???
cfn=(2) yuna::draw
calls=2 0
0 70
ob=(2)
fl=(2) render.rs
fn=(2)
16 50
+2 20
* 5
ob=(3) ???
fn=(3) 0x1234
0 100
totals: 1175
";

    #[test]
    fn callgrind_counts_each_functions_own_instructions_by_object() {
        let read = read_callgrind(CALLGRIND).unwrap();
        assert_eq!(read.command, "/w/bin/base/yuna mcp --manual-clock");
        assert_eq!(read.total, 1175);
        assert_eq!(read.by_object["/nix/store/x-mesa/lib/libgallium.so"], 1000);
        // The 70 of the call is the callee's, counted where its own lines are.
        assert_eq!(read.by_object["/w/bin/base/yuna"], 75);
        assert_eq!(read.by_object["???"], 100);
        assert!(runs(&read.command, Path::new("/w/bin/base/yuna")));
        assert!(!runs(&read.command, Path::new("/w/bin/head/yuna")));
    }

    #[test]
    fn a_command_ran_a_binary_by_its_whole_path() {
        let binary = Path::new("/w/my work/bin/base/yuna");
        assert!(runs("/w/my work/bin/base/yuna mcp --manual-clock", binary));
        assert!(runs("/w/my work/bin/base/yuna", binary));
        // Another binary whose path starts the same, or a path cut at its space.
        assert!(!runs("/w/my work/bin/base/yuna-old mcp", binary));
        assert!(!runs("/w/my mcp", binary));
        assert!(!runs("/w/my work/bin/head/yuna mcp", binary));
        assert!(!runs("", binary));
    }

    #[test]
    fn a_callgrind_file_cut_short_or_miscounted_is_refused() {
        let cut: String = CALLGRIND.lines().take(12).collect::<Vec<_>>().join("\n");
        assert!(read_callgrind(&cut).is_err());
        let wrong = CALLGRIND.replace("summary: 1175", "summary: 1176");
        assert!(read_callgrind(&wrong).unwrap_err().contains("1176"));
        assert!(read_callgrind("").unwrap_err().contains("did not end"));
    }

    #[test]
    fn dhat_tells_the_binarys_own_allocations() {
        let text = r#"{"dhatFileVersion": 2, "cmd": "/w/bin/base/yuna mcp", "pid": 7,
            "pps": [
                {"tb": 100, "tbk": 2, "eb": 64, "fs": [1, 2]},
                {"tb": 4000, "tbk": 10, "eb": 4000, "fs": [1, 3]},
                {"tb": 8, "tbk": 1, "eb": 8, "fs": [1, 4]}
            ],
            "ftbl": ["[root]",
                "0x1: malloc (in /nix/store/v-valgrind/vgpreload_dhat.so)",
                "0x2: yuna::frames::draw (frames.rs:440)",
                "0x3: lp_scene_new (in /nix/store/m-mesa/lib/libgallium.so)",
                "0x4: main (in /w/bin/base/yuna)"]}"#;
        let read = read_dhat(text, Path::new("/w/bin/base/yuna")).unwrap();
        assert_eq!(read.command, "/w/bin/base/yuna mcp");
        assert_eq!(read.blocks, 13);
        assert_eq!(read.bytes, 4108);
        assert_eq!(read.own_blocks, 3);
        assert_eq!(read.end_bytes, 4072);
        assert!(read_dhat("{}", Path::new("/y")).is_err());
        assert!(read_dhat("not json", Path::new("/y")).is_err());
    }

    #[test]
    fn rust_frames_are_told_by_their_source() {
        assert!(is_rust_source(
            "0x47: get_stack_start (stack_overflow.rs:431)"
        ));
        assert!(!is_rust_source(
            "0x47: lp_rast (in /nix/store/m-mesa/libgallium.so)"
        ));
        assert!(!is_rust_source("0x47: f (lp_rast.c:12)"));
        assert!(!is_rust_source("[root]"));
    }
}
