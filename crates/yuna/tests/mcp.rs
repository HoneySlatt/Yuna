//! `yuna mcp` as an agent's MCP client runs it: requests on its standard input, replies on its
//! standard output, nothing else there, and the programs it starts
//! ([ADR-0033](../../../docs/adr/0033-the-mcp-server.md),
//! [ADR-0078](../../../docs/adr/0078-agents-seats-in-one-yuna-frozen-until-stage-2s-core.md)).
//! The protocol's exchanges are tested in the test's own process (`tests/headless/mcp.rs`);
//! here, what needs a process of its own: the programs' environment, their logs, a whole tree of
//! processes ended when Yuna stops, and the accessibility trees of GTK 4 and Qt programs on their
//! seat's accessibility bus, which `yuna mcp` starts and serves as the AT-SPI registry.

#[path = "common/application_environment.rs"]
mod application_environment;
#[path = "common/folders.rs"]
mod folders;

use std::collections::BTreeMap;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::{DirBuilderExt, FileTypeExt, PermissionsExt};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use application_environment::{application_environment, shell_variable};
use folders::fresh_folder;
use rustix::process::{Pid, Signal, kill_process};
use serde_json::{Value, json};
use yuna_bench::procfs::Proc;

/// The empty folder of test `name`'s `yuna mcp`: its runtime folder, its `TMPDIR` and its log.
fn test_folder(name: &str) -> PathBuf {
    fresh_folder(&format!("mcp-test-{name}"))
}

/// The longest a step may take: a debug build of Yuna starts llvmpipe, and CI's runners are slow.
const PATIENCE: Duration = Duration::from_secs(30);

/// The handle of the seat [`Yuna::start`] creates: the first agent's seat.
const SEAT: u64 = 2;

/// A `yuna mcp` process, and its folder: its runtime folder and its log.
struct Yuna {
    child: Child,
    input: Option<ChildStdin>,
    replies: mpsc::Receiver<Value>,
    folder: PathBuf,
    next_id: u64,
}

impl Yuna {
    /// Starts `yuna mcp`, with a `DISPLAY` and a session bus of its own environment that its
    /// programs must not see, and creates a seat, [`SEAT`].
    fn start(name: &str) -> Yuna {
        Yuna::start_in(test_folder(name))
    }

    /// Starts `yuna mcp` as [`Yuna::start`] does, in `folder`, a [`test_folder`] the test filled
    /// before the start: it is not emptied.
    fn start_in(folder: PathBuf) -> Yuna {
        let mut yuna = Yuna::launch_in(folder, None, &[], &[]);
        let created = yuna.call("seat", json!({"action": "create"}));
        assert_eq!(
            created["structuredContent"]["seat"],
            json!(SEAT),
            "{created}"
        );
        yuna
    }

    /// Starts `yuna mcp` with no seat.
    fn start_without_seat(name: &str) -> Yuna {
        Yuna::start_with(name, None)
    }

    /// Starts `yuna mcp` with no seat, and at most `descriptors` open files when given.
    fn start_with(name: &str, descriptors: Option<u32>) -> Yuna {
        Yuna::launch(name, descriptors, &[])
    }

    /// Starts `yuna mcp` with no seat, at most `descriptors` open files when given, and
    /// `variables` added to its environment.
    fn launch(name: &str, descriptors: Option<u32>, variables: &[(&str, &str)]) -> Yuna {
        Yuna::launch_with_options(name, descriptors, variables, &[])
    }

    /// Starts `yuna mcp` as [`Yuna::launch`] does, with `options` after its own.
    fn launch_with_options(
        name: &str,
        descriptors: Option<u32>,
        variables: &[(&str, &str)],
        options: &[&str],
    ) -> Yuna {
        Yuna::launch_in(test_folder(name), descriptors, variables, options)
    }

    /// Starts `yuna mcp` as [`Yuna::launch`] does, with `options` after its own, in `folder`, a
    /// [`test_folder`] that is not emptied.
    fn launch_in(
        folder: PathBuf,
        descriptors: Option<u32>,
        variables: &[(&str, &str)],
        options: &[&str],
    ) -> Yuna {
        for inner in ["runtime", "tmp"] {
            fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(folder.join(inner))
                .expect("a runtime folder and a temporary one");
        }
        let yuna = env!("CARGO_BIN_EXE_yuna");
        let mut command = match descriptors {
            None => Command::new(yuna),
            Some(limit) => {
                let mut shell = Command::new(in_path("sh"));
                shell.args([
                    "-c",
                    "ulimit -n \"$1\" && shift && exec \"$0\" \"$@\"",
                    yuna,
                ]);
                shell.arg(limit.to_string());
                shell
            }
        };
        let mut child = command
            .args(["mcp", "--output", "1920x1080@60,1"])
            .args(options)
            .env("XDG_RUNTIME_DIR", folder.join("runtime"))
            // The seats' homes go there (ADR-0105), removed with the test's folder.
            .env("TMPDIR", folder.join("tmp"))
            .env("YUNA_LOG", "info")
            .env("DISPLAY", ":99")
            .env("DBUS_SESSION_BUS_ADDRESS", "unix:path=/nonexistent")
            .envs(variables.iter().copied())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(fs::File::create(folder.join("yuna.log")).expect("Yuna's log"))
            .spawn()
            .expect("the yuna binary runs");
        let stdout = child.stdout.take().expect("its standard output");
        let (sender, replies) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { return };
                let reply: Value = serde_json::from_str(&line).unwrap_or_else(|_| {
                    panic!("standard output holds a line that is not MCP: {line:?}")
                });
                if sender.send(reply).is_err() {
                    return;
                }
            }
        });
        Yuna {
            input: child.stdin.take(),
            child,
            replies,
            folder,
            next_id: 1,
        }
    }

    fn runtime_dir(&self) -> PathBuf {
        self.folder.join("runtime")
    }

    /// Its `TMPDIR`, where its seats' homes go.
    fn temp_dir(&self) -> PathBuf {
        self.folder.join("tmp")
    }

    /// The folder of its seats' throwaway homes, in its `TMPDIR`
    /// ([ADR-0105](../../../docs/adr/0105-an-agents-programs-on-a-throwaway-home.md)), under
    /// the name it takes when that name is free.
    fn homes_folder(&self) -> PathBuf {
        self.temp_dir()
            .join(format!("yuna-mcp-homes-{}", self.child.id()))
    }

    /// The folder of its seats' sockets and its programs' logs and runtime folders, in its
    /// runtime folder.
    fn programs_folder(&self) -> PathBuf {
        self.runtime_dir()
            .join(format!("yuna-mcp-{}", self.child.id()))
    }

    /// Sends a call of a tool in 2026-07-28, without waiting for its reply; returns its id.
    fn send(&mut self, tool: &str, arguments: Value) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        let request = json!({
            "jsonrpc": "2.0", "id": id, "method": "tools/call",
            "params": {
                "_meta": {
                    "io.modelcontextprotocol/protocolVersion": "2026-07-28",
                    "io.modelcontextprotocol/clientCapabilities": {},
                },
                "name": tool,
                "arguments": arguments,
            }
        });
        let input = self.input.as_mut().expect("its input is open");
        writeln!(input, "{request}").expect("the request is sent");
        id
    }

    /// Waits for it to exit by itself, its input left open; returns its status. Only a debug
    /// build's Yuna, which a panic can be asked of, exits so.
    #[cfg(debug_assertions)]
    fn wait_for_exit(&mut self) -> std::process::ExitStatus {
        let until = Instant::now() + PATIENCE;
        loop {
            if let Some(status) = self.child.try_wait().expect("the process is waited for") {
                return status;
            }
            assert!(Instant::now() < until, "yuna did not stop\n{}", self.log());
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// Calls a tool in 2026-07-28 and returns its result.
    fn call(&mut self, tool: &str, arguments: Value) -> Value {
        let id = self.send(tool, arguments);
        let until = Instant::now() + PATIENCE;
        loop {
            let left = until.saturating_duration_since(Instant::now());
            let reply = self
                .replies
                .recv_timeout(left)
                .unwrap_or_else(|_| panic!("no reply to {tool}\n{}", self.log()));
            if reply["id"] == json!(id) {
                return reply["result"].clone();
            }
        }
    }

    fn log(&self) -> String {
        fs::read_to_string(self.folder.join("yuna.log")).unwrap_or_default()
    }

    /// Closes its input, as an MCP client stopping does, and waits for it to exit.
    fn close_and_wait(&mut self) -> std::process::ExitStatus {
        drop(self.input.take());
        let until = Instant::now() + PATIENCE;
        loop {
            if let Some(status) = self.child.try_wait().expect("the process is waited for") {
                return status;
            }
            assert!(Instant::now() < until, "yuna did not stop\n{}", self.log());
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

impl Drop for Yuna {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = fs::remove_dir_all(&self.folder);
    }
}

/// Waits until `ready` gives a value, or fails with `what`.
fn wait_for<T>(what: &str, mut ready: impl FnMut() -> Option<T>) -> T {
    let until = Instant::now() + PATIENCE;
    loop {
        if let Some(value) = ready() {
            return value;
        }
        assert!(Instant::now() < until, "{what} never came");
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// The parent of process `pid`, from `/proc`; none once it has ended: gone, a zombie (`Z`), or
/// being reaped (`X`), which `/proc` shows for a moment, longer on a loaded machine.
fn parent_of(pid: i32) -> Option<i32> {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let after_name = &stat[stat.rfind(')')? + 1..];
    let mut fields = after_name.split_whitespace();
    let state = fields.next()?;
    if matches!(state, "Z" | "X") {
        return None;
    }
    fields.next()?.parse().ok()
}

/// A program started through `spawn` gets its seat's socket, its seat's X display and cookie,
/// never Yuna's own `DISPLAY` (ADR-0067), a runtime folder of its own, and no session bus
/// (`disabled:`, so that none is launched through X11); its
/// output goes to its log. A process its program
/// leaves behind is adopted by Yuna, as subreaper; `stop` signals a program; and when the
/// client closes Yuna's input, every process under Yuna is ended, one that ignores SIGTERM
/// included, and the programs' folder removed.
#[test]
fn programs_get_a_clean_environment_and_their_whole_tree_ends_with_yuna() {
    let mut yuna = Yuna::start("tree");
    let started = yuna.call(
        "spawn",
        json!({"action": "start", "seat": SEAT, "command": [
            "sh", "-c", "trap '' TERM; sleep 600 & echo grandchild=$!; env; wait"
        ]}),
    );
    assert_eq!(started["isError"], json!(false), "{started}");
    let program = &started["structuredContent"]["program"];
    let shell = program["pid"].as_i64().expect("a pid") as i32;
    let log = PathBuf::from(program["log"].as_str().expect("a log"));
    assert!(log.starts_with(yuna.runtime_dir()), "{}", log.display());

    let output = wait_for("the program's output", || {
        let output = fs::read_to_string(&log).ok()?;
        output.contains("\nXDG_SESSION_TYPE=").then_some(output)
    });
    let variable = |name: &str| {
        output
            .lines()
            .find_map(|line| line.strip_prefix(&format!("{name}=")))
            .map(str::to_owned)
    };
    let display = variable("DISPLAY").expect("its seat's DISPLAY");
    assert_ne!(display, ":99", "Yuna's own DISPLAY reached a program");
    assert!(
        display
            .strip_prefix(':')
            .is_some_and(|n| n.parse::<u32>().is_ok()),
        "{display}"
    );
    let xauthority = PathBuf::from(variable("XAUTHORITY").expect("its seat's XAUTHORITY"));
    assert!(
        xauthority.starts_with(yuna.runtime_dir()),
        "{}",
        xauthority.display()
    );
    assert!(
        xauthority.ends_with("seat-2.x11/Xauthority"),
        "{}",
        xauthority.display()
    );
    // No session bus: none that a program would launch through X11 either.
    assert_eq!(
        variable("DBUS_SESSION_BUS_ADDRESS").as_deref(),
        Some("disabled:")
    );
    assert_eq!(variable("YUNA_SEAT").as_deref(), Some("2"));
    let socket = PathBuf::from(variable("WAYLAND_DISPLAY").expect("WAYLAND_DISPLAY"));
    assert!(socket.is_absolute() && socket.starts_with(yuna.runtime_dir()));
    assert!(socket.ends_with("seat-2"), "{}", socket.display());
    assert!(
        fs::metadata(&socket)
            .expect("the socket")
            .file_type()
            .is_socket()
    );
    let runtime = PathBuf::from(variable("XDG_RUNTIME_DIR").expect("XDG_RUNTIME_DIR"));
    assert!(runtime.starts_with(yuna.runtime_dir()) && runtime != yuna.runtime_dir());
    assert_eq!(
        fs::read_dir(&runtime).expect("its runtime folder").count(),
        0
    );
    let grandchild: i32 = variable("grandchild")
        .and_then(|pid| pid.parse().ok())
        .expect("the grandchild's pid");

    // The shell killed, its child is Yuna's.
    kill_process(Pid::from_raw(shell).expect("a pid"), Signal::KILL).expect("SIGKILL is sent");
    let ended = wait_for("the shell's end", || {
        let listed = yuna.call("spawn", json!({"action": "list", "seat": SEAT}));
        let program = listed["structuredContent"]["programs"][0].clone();
        (program["state"] == json!("killed")).then_some(program)
    });
    assert_eq!(ended["signal"], json!(9));
    let yuna_pid = yuna.child.id() as i32;
    assert_eq!(parent_of(grandchild), Some(yuna_pid));

    // A start that fails leaves the next one free to start.
    let missing = yuna.call(
        "spawn",
        json!({"action": "start", "seat": SEAT, "command": ["no-such-program-x"]}),
    );
    assert_eq!(missing["isError"], json!(true));
    let second = yuna.call(
        "spawn",
        json!({"action": "start", "seat": SEAT, "command": ["sleep", "600"]}),
    );
    assert_eq!(second["isError"], json!(false), "{second}");
    let second_id = second["structuredContent"]["program"]["id"].clone();
    let stopped = yuna.call(
        "spawn",
        json!({"action": "stop", "seat": SEAT, "program": second_id}),
    );
    assert_eq!(stopped["structuredContent"]["signal"], json!("SIGTERM"));
    wait_for("the second program's end", || {
        let listed = yuna.call("spawn", json!({"action": "list", "seat": SEAT}));
        let program = listed["structuredContent"]["programs"][1].clone();
        (program["signal"] == json!(15)).then_some(())
    });
    let again = yuna.call(
        "spawn",
        json!({"action": "stop", "seat": SEAT, "program": second_id}),
    );
    assert_eq!(again["isError"], json!(true));

    let status = yuna.close_and_wait();
    assert!(status.success(), "{status}\n{}", yuna.log());
    wait_for("the grandchild's end", || {
        parent_of(grandchild).is_none().then_some(())
    });
    let left: Vec<_> = fs::read_dir(yuna.runtime_dir())
        .expect("the runtime folder")
        .flatten()
        .map(|entry| entry.file_name())
        .collect();
    assert_eq!(left, Vec::<std::ffi::OsString>::new(), "{}", yuna.log());
}

/// A program found in the test's own `PATH`, by its full path.
fn in_path(program: &str) -> PathBuf {
    std::env::var_os("PATH")
        .iter()
        .flat_map(std::env::split_paths)
        .map(|folder| folder.join(program))
        .find(|path| path.is_file())
        .unwrap_or_else(|| panic!("{program} is not in PATH"))
}

/// An agent starts Alacritty through `spawn`, with the tests' pinned environment, finds its
/// window through `windows`, waiting for it, clicks it and types a command into its shell, which
/// runs it: the path of M1.5's story, through MCP alone.
#[test]
fn an_agent_starts_alacritty_finds_its_window_and_types_into_it() {
    let mut yuna = Yuna::start("alacritty");
    let env: BTreeMap<&str, String> = application_environment(&yuna.folder.join("home"))
        .into_iter()
        .map(|(name, value)| (name, value.to_string_lossy().into_owned()))
        .collect();
    let shell = in_path("sh");
    let started = yuna.call(
        "spawn",
        json!({"action": "start", "seat": SEAT, "env": env, "command": [
            "alacritty", "--option", "cursor.style.blinking=\"Never\"", "--command", shell
        ]}),
    );
    assert_eq!(started["isError"], json!(false), "{started}");
    let found = yuna.call(
        "windows",
        json!({"seat": SEAT, "app_id": "Alacritty", "wait_ms": 50_000}),
    );
    let windows = &found["structuredContent"]["windows"];
    let log = started["structuredContent"]["program"]["log"]
        .as_str()
        .map(|log| fs::read_to_string(log).unwrap_or_default())
        .unwrap_or_default();
    assert_eq!(windows[0]["app_id"], json!("Alacritty"), "{found}\n{log}");
    assert_eq!(windows[0]["output"], json!(1));
    assert_eq!(
        windows[0]["rect"],
        // A new column is half the output: (1920 − 3 × 16) / 2 wide, 1080 − 2 × 16 tall.
        json!({"x": 16.0, "y": 16.0, "width": 936.0, "height": 1048.0})
    );
    assert!(windows[0]["pid"].is_i64());

    let clicked = yuna.call(
        "pointer",
        json!({"action": "click", "seat": SEAT, "x": 400, "y": 300}),
    );
    assert_eq!(
        clicked["structuredContent"]["pointer"]["window"], windows[0]["id"],
        "{clicked}"
    );
    let typed_file = yuna.folder.join("typed");
    let command = format!("touch {}\n", typed_file.display());
    let typed = yuna.call(
        "keyboard",
        json!({"action": "type", "seat": SEAT, "text": command}),
    );
    assert_eq!(
        typed["structuredContent"]["keyboard_window"], windows[0]["id"],
        "{typed}"
    );
    wait_for("the command typed into Alacritty's shell", || {
        typed_file.exists().then_some(())
    });
    let status = yuna.close_and_wait();
    assert!(status.success(), "{status}\n{}", yuna.log());
}

/// The state of program `index`, as `spawn list` gives it.
fn program(yuna: &mut Yuna, index: usize) -> Value {
    let listed = yuna.call("spawn", json!({"action": "list", "seat": SEAT}));
    listed["structuredContent"]["programs"][index].clone()
}

/// A program inherits no descriptor of Yuna's beyond its standard input, output and errors; its
/// log keeps its first `LOG_HEAD` and newest `LOG_TAIL` bytes and counts the rest; a program
/// whose first process left while one it started runs is still stopped, its whole group with it.
#[test]
fn a_program_gets_no_descriptor_of_yunas_a_bounded_log_and_a_group_that_stops() {
    let mut yuna = Yuna::start("bounds");
    // `; true` keeps the shell from becoming `ls`: the folder listed is the shell's.
    let listed = yuna.call(
        "spawn",
        json!({"action": "start", "seat": SEAT, "command": ["sh", "-c", "ls /proc/$$/fd; true"]}),
    );
    let log = PathBuf::from(
        listed["structuredContent"]["program"]["log"]
            .as_str()
            .expect("a log"),
    );
    wait_for("the first program's end", || {
        (program(&mut yuna, 0)["state"] == json!("exited")).then_some(())
    });
    let descriptors = fs::read_to_string(&log).expect("its log");
    assert_eq!(descriptors, "0\n1\n2\n");

    let over = 1 << 17;
    let kept = yuna::mcp::LOG_HEAD + yuna::mcp::LOG_TAIL;
    let flood = format!("head -c {} /dev/zero", kept + over);
    yuna.call(
        "spawn",
        json!({"action": "start", "seat": SEAT, "command": ["sh", "-c", flood]}),
    );
    let flooded = wait_for("the flood's last bytes", || {
        let flooded = program(&mut yuna, 1);
        (flooded["state"] == json!("exited") && flooded["log_dropped"] == json!(over))
            .then_some(flooded)
    });
    assert_eq!(flooded["log_bytes"], json!(kept));

    yuna.call(
        "spawn",
        json!({"action": "start", "seat": SEAT, "command": ["sh", "-c", "sleep 600 & exit 0"]}),
    );
    let left = wait_for("the wrapper's end", || {
        let left = program(&mut yuna, 2);
        (left["state"] == json!("exited")).then_some(left)
    });
    assert_eq!(left["group_running"], json!(true));
    let stopped = yuna.call(
        "spawn",
        json!({"action": "stop", "seat": SEAT, "program": 3}),
    );
    assert_eq!(stopped["isError"], json!(false), "{stopped}");
    wait_for("the group's end", || {
        (program(&mut yuna, 2)["group_running"] == json!(false)).then_some(())
    });
    let again = yuna.call(
        "spawn",
        json!({"action": "stop", "seat": SEAT, "program": 3}),
    );
    assert_eq!(again["isError"], json!(true));

    let status = yuna.close_and_wait();
    assert!(status.success(), "{status}\n{}", yuna.log());
}

/// At most 64 programs run at once; 128 are remembered, the oldest that ended forgotten with
/// its log.
#[test]
fn programs_running_and_remembered_are_bounded() {
    let mut yuna = Yuna::start("many");
    let mut first_log = None;
    for _ in 0..64 {
        let started = yuna.call(
            "spawn",
            json!({"action": "start", "seat": SEAT, "command": ["sleep", "600"]}),
        );
        assert_eq!(started["isError"], json!(false), "{started}");
        first_log.get_or_insert_with(|| {
            PathBuf::from(
                started["structuredContent"]["program"]["log"]
                    .as_str()
                    .expect("a log"),
            )
        });
    }
    let refused = yuna.call(
        "spawn",
        json!({"action": "start", "seat": SEAT, "command": ["sleep", "600"]}),
    );
    assert_eq!(refused["isError"], json!(true));
    yuna.call(
        "spawn",
        json!({"action": "stop", "seat": SEAT, "program": 1}),
    );
    wait_for("the first program's end", || {
        (program(&mut yuna, 0)["state"] == json!("killed")).then_some(())
    });
    let first_log = first_log.expect("the first log");
    assert!(first_log.exists());
    // 63 run; 65 more that end at once make 129 remembered, past the 128 kept.
    for _ in 0..65 {
        let started = yuna.call(
            "spawn",
            json!({"action": "start", "seat": SEAT, "command": ["true"]}),
        );
        assert_eq!(started["isError"], json!(false), "{started}");
        wait_for("a program's end", || {
            let listed = yuna.call("spawn", json!({"action": "list", "seat": SEAT}));
            let programs = listed["structuredContent"]["programs"].as_array()?.clone();
            let running = programs.iter().filter(|p| p["state"] == json!("running"));
            (running.count() == 63).then_some(())
        });
    }
    let listed = yuna.call("spawn", json!({"action": "list", "seat": SEAT}));
    let programs = listed["structuredContent"]["programs"]
        .as_array()
        .expect("a list");
    assert_eq!(programs.len(), 128);
    assert_eq!(programs[0]["id"], json!(2));
    assert!(!first_log.exists());
    let status = yuna.close_and_wait();
    assert!(status.success(), "{status}\n{}", yuna.log());
}

/// `yuna mcp` refuses a standard input that is not a pipe or a socket, rather than spinning on a
/// file it cannot watch.
/// A run that failed, or was killed, keeps its folder; a process id that comes round again gives
/// the next run its name. The next `yuna mcp` must not start on what it left.
#[test]
fn a_folder_a_failed_run_kept_is_emptied_before_yuna_starts() {
    let kept = test_folder("kept");
    let left = kept.join("left-by-a-failed-run");
    fs::write(&left, "").expect("a file of the failed run");
    let mut yuna = Yuna::start("kept");
    assert!(
        !left.exists(),
        "the folder a failed run kept is used as it was"
    );
    let status = yuna.close_and_wait();
    assert!(status.success(), "{status}\n{}", yuna.log());
}

#[test]
fn a_file_as_input_is_refused() {
    let folder = fresh_folder("mcp-file");
    let requests = folder.join("requests");
    fs::write(&requests, "{}\n").expect("a file of requests");
    let output = Command::new(env!("CARGO_BIN_EXE_yuna"))
        .args(["mcp", "--output", "640x480@60,1"])
        .env("XDG_RUNTIME_DIR", &folder)
        .stdin(fs::File::open(&requests).expect("the file"))
        .output()
        .expect("the yuna binary runs");
    let left = fs::read_dir(&folder)
        .map(|entries| entries.count())
        .unwrap_or(0);
    let _ = fs::remove_dir_all(&folder);
    // The request file alone: no socket, no programs' folder left.
    assert_eq!(left, 1);
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("not a file"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// Whether process `pid` lives: neither gone nor a zombie.
fn alive(pid: i32) -> bool {
    parent_of(pid).is_some()
}

/// A number the program `log` printed as `name=<number>`, once it did.
fn printed(log: &std::path::Path, name: &str) -> i32 {
    wait_for(name, || {
        fs::read_to_string(log).ok()?.lines().find_map(|line| {
            line.strip_prefix(&format!("{name}="))
                .and_then(|pid| pid.parse().ok())
        })
    })
}

/// Closing a seat ([ADR-0095](../../../docs/adr/0095-an-agents-seat-through-mcp-its-tools-input-keyboard-layouts-and-closing.md)) ends its
/// processes and those alone, and answers once they are gone: its program, which ignores
/// SIGTERM and gets SIGKILL 2 s later, and a daemon that left its parents and its group, found
/// by `YUNA_SEAT`. Another seat's programs run on, one that joined the closed seat's process
/// group included, and `spawn stop` refuses another seat's program. A daemon that also empties its environment
/// escapes the closing, as the ADR says, but not Yuna's stopping. The seat's socket goes.
#[test]
fn closing_a_seat_ends_its_processes_and_no_other_seats() {
    let mut yuna = Yuna::start("close");
    let other = yuna.call("seat", json!({"action": "create"}));
    assert_eq!(other["structuredContent"]["seat"], json!(3), "{other}");
    let socket_2 = PathBuf::from(
        yuna.call("seat", json!({"action": "list"}))["structuredContent"]["seats"][0]["socket"]
            .as_str()
            .expect("a socket")
            .to_owned(),
    );
    assert!(
        fs::metadata(&socket_2)
            .expect("its socket")
            .file_type()
            .is_socket()
    );

    // A daemon of its own session, its parent gone: Yuna, subreaper, adopts it.
    let script = yuna.folder.join("daemons.sh");
    fs::write(
        &script,
        "trap '' TERM\n\
         (setsid sh -c 'echo daemon=$$; exec sleep 600' &)\n\
         S=$(command -v sleep)\n\
         (env -i \"$(command -v setsid)\" \"$(command -v sh)\" -c \"echo escaped=\\$\\$; exec $S 600\" &)\n\
         echo shell=$$\n\
         while true; do sleep 1; done\n",
    )
    .expect("the script");
    let started = yuna.call(
        "spawn",
        json!({"action": "start", "seat": SEAT, "command": ["sh", script]}),
    );
    let log = PathBuf::from(
        started["structuredContent"]["program"]["log"]
            .as_str()
            .unwrap_or_else(|| panic!("{started}")),
    );
    let shell = printed(&log, "shell");
    let daemon = printed(&log, "daemon");
    let escaped = printed(&log, "escaped");
    let yuna_pid = yuna.child.id() as i32;
    wait_for("the daemons' adoption", || {
        (parent_of(daemon) == Some(yuna_pid) && parent_of(escaped) == Some(yuna_pid)).then_some(())
    });
    let neighbour = yuna.call(
        "spawn",
        json!({"action": "start", "seat": 3, "command": ["sleep", "600"]}),
    );
    let neighbour_id = neighbour["structuredContent"]["program"]["id"].clone();
    let neighbour = neighbour["structuredContent"]["program"]["pid"]
        .as_i64()
        .expect("a pid") as i32;
    let not_ours = yuna.call(
        "spawn",
        json!({"action": "stop", "seat": SEAT, "program": neighbour_id}),
    );
    assert_eq!(not_ours["isError"], json!(true), "another seat's program");
    // A process of seat 3 in the process group seat 2's program leads, as a pid reused by
    // another seat would be: its `YUNA_SEAT` says it is seat 3's.
    let joiner = yuna.call(
        "spawn",
        json!({"action": "start", "seat": 3, "command": ["python3", "-c", format!(
            "import os, time\nos.setpgid(0, {shell})\nprint('joined=%d' % os.getpid(), flush=True)\ntime.sleep(600)"
        )]}),
    );
    let joiner_log = PathBuf::from(
        joiner["structuredContent"]["program"]["log"]
            .as_str()
            .unwrap_or_else(|| panic!("{joiner}")),
    );
    let joiner = printed(&joiner_log, "joined");

    let began = Instant::now();
    let closed = yuna.call("seat", json!({"action": "close", "seat": SEAT}));
    let took = began.elapsed();
    let result = &closed["structuredContent"];
    assert_eq!(result["closed"], json!(SEAT), "{closed}");
    let ended: Vec<i64> = result["ended"]
        .as_array()
        .expect("a list")
        .iter()
        .filter_map(Value::as_i64)
        .collect();
    assert!(
        ended.contains(&i64::from(shell)) && ended.contains(&i64::from(daemon)),
        "{result}"
    );
    assert!(!ended.contains(&i64::from(escaped)) && !ended.contains(&i64::from(neighbour)));
    assert_eq!(result["left_running"], json!([]));
    // The shell ignores SIGTERM: SIGKILL 2 s later, and the answer soon after.
    assert!(
        took >= Duration::from_secs(2) && took < Duration::from_secs(4),
        "{took:?}"
    );
    assert!(!alive(shell) && !alive(daemon));
    assert!(alive(neighbour) && alive(escaped) && alive(joiner));
    assert!(!socket_2.exists(), "its socket is removed");
    let gone = yuna.call("spawn", json!({"action": "list", "seat": SEAT}));
    assert_eq!(gone["isError"], json!(true));
    let seats = yuna.call("seat", json!({"action": "list"}));
    assert_eq!(seats["structuredContent"]["seats"][0]["seat"], json!(3));

    let status = yuna.close_and_wait();
    assert!(status.success(), "{status}\n{}", yuna.log());
    wait_for("every process's end", || {
        (!alive(escaped) && !alive(neighbour) && !alive(joiner)).then_some(())
    });
}

/// Whether a Wayland server answers on `stream`: a `wl_display.sync` written by hand, object 1,
/// opcode 0, a message of 12 bytes creating object 2, gets `wl_callback.done` back.
fn answers_wayland(stream: &mut std::os::unix::net::UnixStream) -> bool {
    use std::io::Read;
    let mut request = Vec::new();
    for word in [1_u32, 12 << 16, 2] {
        request.extend_from_slice(&word.to_ne_bytes());
    }
    stream
        .set_read_timeout(Some(PATIENCE))
        .expect("a read timeout");
    let mut reply = [0; 64];
    stream.write_all(&request).is_ok() && matches!(stream.read(&mut reply), Ok(1..))
}

/// A seat's programs that open connections until Yuna runs out of descriptors never stop Yuna
/// (ADR-0080): its seat's socket rests, Yuna keeps answering, and once they close, a new client
/// connects again.
#[test]
fn running_out_of_descriptors_pauses_a_seats_socket_and_stops_nothing() {
    let mut yuna = Yuna::start_with("descriptors", Some(96));
    let created = yuna.call("seat", json!({"action": "create"}));
    let socket = PathBuf::from(
        created["structuredContent"]["socket"]
            .as_str()
            .unwrap_or_else(|| panic!("{created}")),
    );
    let flood: Vec<std::os::unix::net::UnixStream> = (0..150)
        .filter_map(|_| std::os::unix::net::UnixStream::connect(&socket).ok())
        .collect();
    assert!(flood.len() > 96, "the kernel's backlog holds the rest");
    wait_for("the socket's pause", || {
        yuna.log().contains("a seat's socket pauses").then_some(())
    });
    let listed = yuna.call("seat", json!({"action": "list"}));
    assert_eq!(listed["structuredContent"]["seats"][0]["seat"], json!(SEAT));
    drop(flood);
    let mut client = wait_for("a new connection", || {
        std::os::unix::net::UnixStream::connect(&socket).ok()
    });
    assert!(answers_wayland(&mut client), "{}", yuna.log());
    let status = yuna.close_and_wait();
    assert!(status.success(), "{status}\n{}", yuna.log());
}

/// The variables that make a debug build's Yuna panic on `clock`'s call
/// ([`yuna::mcp::PANIC_ON_TOOL`]), with no backtrace, which a debug build takes a while to write:
/// the time measured is the fatal path's alone.
#[cfg(debug_assertions)]
const PANIC_ON_CLOCK: &[(&str, &str)] =
    &[(yuna::mcp::PANIC_ON_TOOL, "clock"), ("RUST_BACKTRACE", "0")];

/// Whether Yuna's errors hold the panic `clock`'s call asked for, as the standard hook writes it.
#[cfg(debug_assertions)]
fn holds_the_panic(log: &str) -> bool {
    log.contains("panicked at")
        && log.contains("asks Yuna's state thread to panic when `clock` is called")
}

/// A panic of Yuna's state thread stops it at once, with status 101
/// ([ADR-0080](../../../docs/adr/0080-what-survives-a-failure-and-yunas-own-panics.md)): every
/// process under it ends first, a program whose whole tree ignores SIGTERM included, an orphan
/// Yuna adopted with it, SIGKILL coming `END_GRACE` after SIGTERM; its folder in
/// `XDG_RUNTIME_DIR` goes; and its errors hold the panic's message. The panic is asked through
/// `YUNA_PANIC_ON_TOOL`, which a debug build alone has.
#[cfg(debug_assertions)]
#[test]
fn a_panic_of_the_state_ends_every_program_and_removes_the_folder() {
    use yuna::mcp::{END_GRACE, KILL_WAIT};
    let mut yuna = Yuna::launch("panic", None, PANIC_ON_CLOCK);
    let created = yuna.call("seat", json!({"action": "create"}));
    assert_eq!(
        created["structuredContent"]["seat"],
        json!(SEAT),
        "{created}"
    );
    let started = yuna.call(
        "spawn",
        json!({"action": "start", "seat": SEAT, "command": [
            "sh", "-c",
            "trap '' TERM; sleep 600 & echo child=$!; (sleep 600 & echo orphan=$!); \
             echo shell=$$; wait"
        ]}),
    );
    assert_eq!(started["isError"], json!(false), "{started}");
    let log = PathBuf::from(
        started["structuredContent"]["program"]["log"]
            .as_str()
            .expect("a log"),
    );
    let tree = [
        printed(&log, "shell"),
        printed(&log, "child"),
        printed(&log, "orphan"),
    ];
    let yuna_pid = yuna.child.id() as i32;
    wait_for("the orphan's adoption", || {
        (parent_of(tree[2]) == Some(yuna_pid)).then_some(())
    });
    let folder = yuna.runtime_dir().join(format!("yuna-mcp-{yuna_pid}"));
    assert!(folder.is_dir(), "{}", folder.display());

    let began = Instant::now();
    yuna.send("clock", json!({}));
    let status = yuna.wait_for_exit();
    let took = began.elapsed();
    eprintln!("Yuna stopped {} ms after the call", took.as_millis());
    assert_eq!(status.code(), Some(101), "{status}\n{}", yuna.log());
    // The tree ignores SIGTERM: it ends at SIGKILL, END_GRACE later, and is reaped at once.
    assert!(
        took >= END_GRACE && took < END_GRACE + KILL_WAIT + Duration::from_secs(1),
        "{took:?}"
    );
    for pid in tree {
        assert!(!alive(pid), "process {pid} outlived Yuna");
    }
    assert!(!folder.exists(), "{} is left", folder.display());
    assert!(holds_the_panic(&yuna.log()), "{}", yuna.log());
}

/// A panic with no program running stops Yuna at once, with nothing to wait for; its folder,
/// which holds a seat's socket, goes.
#[cfg(debug_assertions)]
#[test]
fn a_panic_with_no_program_stops_yuna_at_once() {
    use yuna::mcp::END_GRACE;
    let mut yuna = Yuna::launch("panic-alone", None, PANIC_ON_CLOCK);
    let created = yuna.call("seat", json!({"action": "create"}));
    let socket = PathBuf::from(
        created["structuredContent"]["socket"]
            .as_str()
            .unwrap_or_else(|| panic!("{created}")),
    );
    let folder = yuna
        .runtime_dir()
        .join(format!("yuna-mcp-{}", yuna.child.id()));
    assert!(socket.starts_with(&folder), "{}", socket.display());

    let began = Instant::now();
    yuna.send("clock", json!({}));
    let status = yuna.wait_for_exit();
    let took = began.elapsed();
    eprintln!("Yuna stopped {} ms after the call", took.as_millis());
    assert_eq!(status.code(), Some(101), "{status}\n{}", yuna.log());
    assert!(took < END_GRACE / 2, "{took:?}");
    assert!(!folder.exists(), "{} is left", folder.display());
    assert!(holds_the_panic(&yuna.log()), "{}", yuna.log());
}

/// `events`' answer to `arguments`, which must not fail.
fn events(yuna: &mut Yuna, arguments: Value) -> Value {
    let result = yuna.call("events", arguments);
    assert_eq!(result["isError"], json!(false), "{result}");
    result["structuredContent"].clone()
}

/// The id of the program a `spawn start` started.
fn program_id(started: &Value) -> u64 {
    started["structuredContent"]["program"]["id"]
        .as_u64()
        .unwrap_or_else(|| panic!("{started}"))
}

/// A Wayland client written by hand, in Python's standard library alone: it binds
/// `wl_compositor` and `wl_subcompositor`, makes two surfaces, and makes the first a subsurface
/// of the second twice, a role it already has, then reads until the connection closes.
const ROLE_TWICE: &str = r#"
import os, socket, struct, sys

def message(target, opcode, payload=b""):
    return struct.pack("=II", target, (8 + len(payload)) << 16 | opcode) + payload

def string(text):
    data = text.encode() + b"\0"
    return struct.pack("=I", len(data)) + data + b"\0" * (-len(data) % 4)

wayland = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
wayland.connect(os.environ["WAYLAND_DISPLAY"])
# wl_display.get_registry, object 2, then wl_display.sync, object 3.
wayland.sendall(message(1, 1, struct.pack("=I", 2)) + message(1, 0, struct.pack("=I", 3)))
names, data, synced = {}, b"", False
while not synced:
    chunk = wayland.recv(65536)
    if not chunk:
        sys.exit("the connection closed before the globals came")
    data += chunk
    while len(data) >= 8:
        target, word = struct.unpack("=II", data[:8])
        size = word >> 16
        if len(data) < size:
            break
        body, data = data[8:size], data[size:]
        if target == 2 and word & 0xFFFF == 0:
            name, length = struct.unpack("=II", body[:8])
            names[body[8:8 + length - 1].decode()] = name
        synced = synced or target == 3

def bind(interface, version, new_id):
    return message(2, 0, struct.pack("=I", names[interface]) + string(interface)
                   + struct.pack("=II", version, new_id))

requests = bind("wl_compositor", 4, 4) + bind("wl_subcompositor", 1, 5)
requests += message(4, 0, struct.pack("=I", 6)) + message(4, 0, struct.pack("=I", 7))
requests += message(5, 1, struct.pack("=III", 8, 6, 7)) + message(5, 1, struct.pack("=III", 9, 6, 7))
wayland.sendall(requests)
while wayland.recv(65536):
    pass
print("closed", flush=True)
"#;

/// A program whose Wayland client breaks the protocol is told, through `events`, as
/// `client_left` with the error Yuna sent, its pid and its program's id, waited for; `spawn list`
/// then gives the program that error, and `events` its exit.
#[test]
fn a_programs_protocol_error_is_told_and_kept_on_the_program() {
    let mut yuna = Yuna::start("protocol-error");
    let script = yuna.folder.join("role_twice.py");
    fs::write(&script, ROLE_TWICE).expect("the client's script");
    let start = events(&mut yuna, json!({"seat": SEAT}))["next"].clone();
    let started = yuna.call(
        "spawn",
        json!({"action": "start", "seat": SEAT, "command": ["python3", "-I", script]}),
    );
    let id = program_id(&started);
    let pid = started["structuredContent"]["program"]["pid"].clone();
    let told = events(
        &mut yuna,
        json!({"seat": SEAT, "after": start, "kinds": ["client_left"], "wait_ms": 20_000}),
    );
    let left = &told["events"][0];
    assert_eq!(left["reason"], json!("protocol_error"), "{told}");
    assert_eq!(left["program"], json!(id), "{told}");
    assert_eq!(left["pid"], pid, "{told}");
    let error = left["error"].clone();
    assert_eq!(error["interface"], json!("wl_subcompositor"), "{told}");
    assert_eq!(error["object"], json!(5));
    // `wl_subcompositor.error.bad_surface`.
    assert_eq!(error["code"], json!(0));
    assert!(
        error["message"]
            .as_str()
            .is_some_and(|text| !text.is_empty()),
        "{told}"
    );
    let exited = events(
        &mut yuna,
        json!({"seat": SEAT, "after": start, "kinds": ["program_exited"], "wait_ms": 20_000}),
    );
    assert_eq!(
        exited["events"][0],
        json!({"seq": exited["events"][0]["seq"], "time_ns": exited["events"][0]["time_ns"],
               "kind": "program_exited", "program": id, "exit_code": 0, "signal": null}),
        "{exited}\n{}",
        yuna.log()
    );
    assert_eq!(program(&mut yuna, 0)["protocol_error"], error);
    let status = yuna.close_and_wait();
    assert!(status.success(), "{status}\n{}", yuna.log());
}

/// `spawn log` reads a program's output by offsets, and its newest bytes by default; after
/// 3 MiB of output it keeps the first 256 KiB and the last 1 MiB, counts the rest as dropped,
/// and never gives bytes across what it dropped.
#[test]
fn a_programs_log_is_read_by_offsets_and_keeps_its_head_and_tail() {
    use yuna::mcp::{DEFAULT_LOG_READ, LOG_HEAD, LOG_TAIL, MAX_LOG_READ};
    let mut yuna = Yuna::start("log");
    let flood = 3 << 20;
    let started = yuna.call(
        "spawn",
        json!({"action": "start", "seat": SEAT, "command": ["sh", "-c", format!(
            "printf 'first line\\n'; head -c {flood} /dev/zero | tr '\\000' x; printf '\\nlast line\\n'"
        )]}),
    );
    let id = program_id(&started);
    let read = |yuna: &mut Yuna, extra: Value| {
        let mut arguments = json!({"action": "log", "seat": SEAT, "program": id});
        for (name, value) in extra.as_object().cloned().unwrap_or_default() {
            arguments[name] = value;
        }
        let result = yuna.call("spawn", arguments);
        assert_eq!(result["isError"], json!(false), "{result}");
        result["structuredContent"].clone()
    };
    let size = 11 + flood + 11;
    let tail = wait_for("the end of the program's output", || {
        let tail = read(&mut yuna, json!({}));
        (tail["ended"] == json!(true)).then_some(tail)
    });
    assert_eq!(tail["size"], json!(size));
    assert_eq!(tail["dropped"], json!(size - LOG_HEAD - LOG_TAIL));
    assert_eq!(tail["to"], json!(size));
    assert_eq!(tail["from"], json!(size - DEFAULT_LOG_READ));
    let text = tail["text"].as_str().expect("a text");
    assert!(
        text.ends_with("xxx\nlast line\n"),
        "{}",
        &text[text.len() - 40..]
    );

    let head = read(&mut yuna, json!({"from": 0, "max_bytes": 11}));
    assert_eq!(head["text"], json!("first line\n"));
    assert_eq!(
        (head["from"].clone(), head["to"].clone()),
        (json!(0), json!(11))
    );
    // The head's last bytes, never those after it, which were dropped.
    let edge = read(&mut yuna, json!({"from": LOG_HEAD - 5, "max_bytes": 100}));
    assert_eq!(edge["text"], json!("xxxxx"));
    assert_eq!(edge["to"], json!(LOG_HEAD));
    // A start in what was dropped moves to the first byte kept after it.
    let gap = read(&mut yuna, json!({"from": LOG_HEAD}));
    assert_eq!(gap["from"], json!(size - LOG_TAIL));
    assert_eq!(gap["to"], json!(size - LOG_TAIL + DEFAULT_LOG_READ));
    let whole_tail = read(&mut yuna, json!({"max_bytes": MAX_LOG_READ}));
    assert_eq!(whole_tail["from"], json!(size - LOG_TAIL));
    assert_eq!(
        whole_tail["text"].as_str().map(str::len),
        Some(LOG_TAIL as usize)
    );
    let past = read(&mut yuna, json!({"from": size + 10}));
    assert_eq!(
        (past["from"].clone(), past["text"].clone()),
        (json!(size), json!(""))
    );
    for refused in [json!(0), json!(MAX_LOG_READ + 1)] {
        let call = json!({"action": "log", "seat": SEAT, "program": id, "max_bytes": refused});
        assert_eq!(yuna.call("spawn", call)["isError"], json!(true));
    }
    let unknown = json!({"action": "log", "seat": SEAT, "program": 99});
    assert_eq!(yuna.call("spawn", unknown)["isError"], json!(true));
    let listed = program(&mut yuna, 0);
    assert_eq!(listed["log_bytes"], json!(LOG_HEAD + LOG_TAIL));
    assert_eq!(listed["log_dropped"], tail["dropped"]);
    let status = yuna.close_and_wait();
    assert!(status.success(), "{status}\n{}", yuna.log());
}

/// The environment a test's real application gets, through `spawn`'s `env`: the tests' pinned
/// one, GTK's cursor that never blinks and animations off, and `extra` over it.
fn pinned_environment(yuna: &Yuna, extra: &[(&str, String)]) -> BTreeMap<String, String> {
    let home = yuna.folder.join("home");
    let settings = home.join("config/gtk-4.0");
    fs::create_dir_all(&settings).expect("GTK's settings folder");
    fs::write(
        settings.join("settings.ini"),
        "[Settings]\ngtk-cursor-blink=false\ngtk-enable-animations=false\n",
    )
    .expect("GTK's settings");
    let mut env: BTreeMap<String, String> = application_environment(&home)
        .into_iter()
        .map(|(name, value)| (name.to_owned(), value.to_string_lossy().into_owned()))
        .collect();
    env.extend(
        extra
            .iter()
            .map(|(name, value)| ((*name).to_owned(), value.clone())),
    );
    env
}

/// GTK 4's variables: Wayland, its Vulkan renderer on lavapipe, as `tests/applications.rs` runs it.
fn gtk_variables() -> Vec<(&'static str, String)> {
    vec![
        ("GDK_BACKEND", "wayland".to_owned()),
        ("GSK_RENDERER", "vulkan".to_owned()),
    ]
}

/// Qt's QML runner's variables on `platform`, `wayland` or `xcb`, as `tests/applications.rs`
/// gives them.
fn qt_variables(platform: &str) -> Vec<(&'static str, String)> {
    vec![
        ("QT_QPA_PLATFORM", platform.to_owned()),
        ("QT_PLUGIN_PATH", shell_variable("YUNA_QT_PLUGINS")),
        ("QML_IMPORT_PATH", shell_variable("YUNA_QML_IMPORTS")),
        ("QSG_RHI_BACKEND", "opengl".to_owned()),
        ("QT_FORCE_STDERR_LOGGING", "1".to_owned()),
    ]
}

/// The QML scene of the accessibility tests: a button named "Press me" that becomes "Pressed".
fn accessible_scene() -> String {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/accessible.qml")
        .display()
        .to_string()
}

impl Yuna {
    /// Starts `command` on `seat` through `spawn`, with `env`, on the seat's accessibility bus
    /// when `accessibility`; gives the program, as `spawn` tells it.
    fn start_program(
        &mut self,
        seat: u64,
        command: &[&str],
        env: &BTreeMap<String, String>,
        accessibility: bool,
    ) -> Value {
        let started = self.call(
            "spawn",
            json!({"action": "start", "seat": seat, "command": command, "env": env,
                   "accessibility": accessibility}),
        );
        assert_eq!(
            started["isError"],
            json!(false),
            "{started}\n{}",
            self.log()
        );
        started["structuredContent"]["program"].clone()
    }

    /// Calls `accessibility` and gives its result, which must not have failed.
    fn accessible_nodes(&mut self, arguments: Value) -> Value {
        let found = self.call("accessibility", arguments.clone());
        assert_eq!(
            found["isError"],
            json!(false),
            "{arguments}: {found}\n{}",
            self.log()
        );
        found["structuredContent"].clone()
    }
}

/// The first node of `found`, an `accessibility` result, and its rectangle's centre, which a
/// pointer clicks.
fn first_node_centre(found: &Value, what: &str) -> (Value, f64, f64) {
    let node = found["nodes"][0].clone();
    let rect = &node["rect"];
    let (Some(x), Some(y), Some(width), Some(height)) = (
        rect["x"].as_f64(),
        rect["y"].as_f64(),
        rect["width"].as_f64(),
        rect["height"].as_f64(),
    ) else {
        panic!("{what} has no rectangle: {found}");
    };
    (node, x + width / 2.0, y + height / 2.0)
}

/// The `dbus-daemon` processes whose parent is `yuna`: its seats' accessibility buses.
fn daemons_of(yuna: i32) -> Vec<i32> {
    let Ok(entries) = fs::read_dir("/proc") else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|entry| {
            let pid: i32 = entry.file_name().to_str()?.parse().ok()?;
            let name = fs::read_to_string(entry.path().join("comm")).ok()?;
            (name.trim() == "dbus-daemon" && parent_of(pid) == Some(yuna)).then_some(pid)
        })
        .collect()
}

/// A variable program `log` printed as `name=value` with `env`, or none once it printed its
/// environment without it.
fn printed_variable(log: &std::path::Path, name: &str) -> Option<String> {
    let output = wait_for("the program's environment", || {
        let output = fs::read_to_string(log).ok()?;
        output.contains("\nYUNA_SEAT=").then_some(output)
    });
    output
        .lines()
        .find_map(|line| line.strip_prefix(&format!("{name}=")))
        .map(str::to_owned)
}

/// An agent finds a GTK 4 button through MCP alone and presses it: gtk4-demo's Dialogs window,
/// started with accessibility, has a button named "Message Dialog", found by a part of its name
/// in any case and its role, with a rectangle on its output; `pointer` clicks the rectangle's
/// centre, and the dialog the button opens, "Test message", is read through `accessibility`
/// again, a window of its own. The same program started without accessibility is not on the bus,
/// and the tool says so.
#[test]
fn an_agent_finds_a_gtk_button_by_its_name_and_presses_it_at_its_centre() {
    let mut yuna = Yuna::start("a11y-gtk");
    let env = pinned_environment(&yuna, &gtk_variables());
    let demo = yuna.start_program(SEAT, &["gtk4-demo", "--run=dialog"], &env, true);
    assert_eq!(demo["accessibility"], json!(true), "{demo}");
    let found = yuna.accessible_nodes(json!({
        "seat": SEAT, "role": "button", "name": "message dialog", "wait_ms": 40_000
    }));
    let (button, x, y) = first_node_centre(&found, "the Message Dialog button");
    // GTK names it without its mnemonic's underscore.
    assert_eq!(button["name"], json!("Message Dialog"), "{found}");
    assert_eq!(button["app"], demo["pid"], "{found}");
    assert!(
        button["states"]
            .as_array()
            .is_some_and(|states| states.contains(&json!("showing"))),
        "{found}"
    );
    let window = button["window"].clone();
    assert!(window.is_u64(), "{found}");
    eprintln!("GTK 4's button: {button}");

    let windows_before =
        yuna.call("windows", json!({"seat": SEAT}))["structuredContent"]["windows"]
            .as_array()
            .map_or(0, Vec::len);
    let clicked = yuna.call(
        "pointer",
        json!({"action": "click", "seat": SEAT, "x": x, "y": y, "output": button["output"]}),
    );
    assert_eq!(
        clicked["structuredContent"]["pointer"]["window"], window,
        "{clicked}"
    );
    let dialog = yuna.accessible_nodes(json!({
        "seat": SEAT, "name": "Test message", "wait_ms": 30_000
    }));
    assert!(
        !dialog["nodes"].as_array().is_none_or(Vec::is_empty),
        "{dialog}"
    );
    let windows_after = wait_for("the dialog's window", || {
        let listed = yuna.call("windows", json!({"seat": SEAT}));
        let count = listed["structuredContent"]["windows"]
            .as_array()
            .map_or(0, Vec::len);
        (count > windows_before).then_some(count)
    });
    assert!(windows_after > windows_before);

    // The dialog's own button, in a window of its own that floats over the first: its rectangle
    // is placed from that window, and its click closes the dialog.
    let cancel = yuna.accessible_nodes(json!({
        "seat": SEAT, "role": "button", "name": "cancel", "wait_ms": 30_000
    }));
    let (cancel_button, x, y) = first_node_centre(&cancel, "the dialog's Cancel button");
    eprintln!("GTK 4's dialog's button: {cancel_button}");
    assert_ne!(cancel_button["window"], window, "{cancel}");
    let clicked = yuna.call(
        "pointer",
        json!({"action": "click", "seat": SEAT, "x": x, "y": y, "output": cancel_button["output"]}),
    );
    assert_eq!(
        clicked["structuredContent"]["pointer"]["window"], cancel_button["window"],
        "{clicked}"
    );
    wait_for("the dialog's end", || {
        let listed = yuna.call("windows", json!({"seat": SEAT}));
        let count = listed["structuredContent"]["windows"]
            .as_array()
            .map_or(0, Vec::len);
        (count == windows_before).then_some(())
    });

    // Without accessibility, the same program is not on the bus, and is said not to be.
    let plain = yuna.start_program(SEAT, &["gtk4-demo", "--run=dialog"], &env, false);
    let plain_pid = plain["pid"].clone();
    wait_for("the plain demo's window", || {
        let listed = yuna.call("windows", json!({"seat": SEAT, "pid": plain_pid}));
        (!listed["structuredContent"]["windows"]
            .as_array()?
            .is_empty())
        .then_some(())
    });
    let refused = yuna.call("accessibility", json!({"seat": SEAT, "pid": plain_pid}));
    assert_eq!(refused["isError"], json!(true), "{refused}");
    let text = refused["content"][0]["text"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    assert!(
        text.contains("is not on seat 2's accessibility bus")
            && text.contains("accessibility: true"),
        "{text}"
    );
    let status = yuna.close_and_wait();
    assert!(status.success(), "{status}\n{}", yuna.log());
}

/// The state letter `/proc/<pid>/stat` gives a process: `T` while it is stopped.
fn state_of(pid: i32) -> Option<char> {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    stat[stat.rfind(')')? + 1..].trim_start().chars().next()
}

/// The CPU time process `pid` has used, in clock ticks: its user and system times.
fn cpu_ticks(pid: i32) -> u64 {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).expect("the process");
    let after_name = &stat[stat.rfind(')').expect("a name") + 1..];
    let fields: Vec<&str> = after_name.split_whitespace().collect();
    fields[11].parse::<u64>().expect("utime") + fields[12].parse::<u64>().expect("stime")
}

/// A frozen program stops: its process is stopped and its output stops growing, `spawn list`
/// gives it a run_percent of 0; thawed, it runs and writes again.
#[test]
fn a_frozen_program_stops_and_thaws() {
    let mut yuna = Yuna::start("freeze");
    let started = yuna.call(
        "spawn",
        json!({"action": "start", "seat": SEAT, "command": [
            "sh", "-c", "while :; do echo tick; sleep 0.02; done"
        ]}),
    );
    let id = program_id(&started);
    let pid = started["structuredContent"]["program"]["pid"]
        .as_i64()
        .expect("a pid") as i32;
    let written = |yuna: &mut Yuna| program(yuna, 0)["log_bytes"].as_u64().unwrap_or(0);
    wait_for("its first ticks", || (written(&mut yuna) > 0).then_some(()));
    let frozen = yuna.call(
        "spawn",
        json!({"action": "freeze", "seat": SEAT, "program": id}),
    );
    assert_eq!(
        frozen["structuredContent"]["program"]["run_percent"],
        json!(0),
        "{frozen}"
    );
    wait_for("its stop", || (state_of(pid) == Some('T')).then_some(()));
    std::thread::sleep(Duration::from_millis(100));
    let before = written(&mut yuna);
    std::thread::sleep(Duration::from_millis(400));
    assert_eq!(written(&mut yuna), before, "a frozen program wrote");
    assert_eq!(state_of(pid), Some('T'));
    let thawed = yuna.call(
        "spawn",
        json!({"action": "thaw", "seat": SEAT, "program": id}),
    );
    assert_eq!(
        thawed["structuredContent"]["program"]["run_percent"],
        json!(100)
    );
    wait_for("its ticks again", || {
        (written(&mut yuna) > before).then_some(())
    });
    let unknown = yuna.call(
        "spawn",
        json!({"action": "freeze", "seat": SEAT, "program": 9}),
    );
    assert_eq!(unknown["isError"], json!(true));
    let status = yuna.close_and_wait();
    assert!(status.success(), "{status}\n{}", yuna.log());
}

/// Finds Qt's button named "Press me" on `seat`, clicks its centre through `pointer`, and reads
/// its new name, "Pressed", through `accessibility`: Qt's state change seen again.
fn press_qt_button(yuna: &mut Yuna, seat: u64, qml: &Value) -> Value {
    let found = yuna.accessible_nodes(json!({
        "seat": seat, "role": "button", "name": "Press me", "wait_ms": 40_000
    }));
    let (button, x, y) = first_node_centre(&found, "Qt's button");
    assert_eq!(button["app"], qml["pid"], "{found}");
    eprintln!("Qt's button on seat {seat}: {button}");
    let clicked = yuna.call(
        "pointer",
        json!({"action": "click", "seat": seat, "x": x, "y": y, "output": button["output"]}),
    );
    assert_eq!(
        clicked["structuredContent"]["pointer"]["window"], button["window"],
        "{clicked}"
    );
    let pressed = yuna.accessible_nodes(json!({
        "seat": seat, "role": "button", "name": "Pressed", "wait_ms": 30_000
    }));
    assert_eq!(pressed["nodes"][0]["name"], json!("Pressed"), "{pressed}");
    button
}

/// The same for Qt 6 on Wayland: a QML button away from its window's corner, whose name changes
/// once pressed.
#[test]
fn an_agent_finds_a_qt_button_and_reads_its_new_name_once_pressed() {
    let mut yuna = Yuna::start("a11y-qt");
    let env = pinned_environment(&yuna, &qt_variables("wayland"));
    let scene = accessible_scene();
    let qml = yuna.start_program(SEAT, &["qml", &scene], &env, true);
    let button = press_qt_button(&mut yuna, SEAT, &qml);
    // The QML scene places it at (200, 180) in its window, 240 × 80.
    assert_eq!(button["rect"]["width"], json!(240), "{button}");
    assert_eq!(button["rect"]["height"], json!(80), "{button}");
    let status = yuna.close_and_wait();
    assert!(status.success(), "{status}\n{}", yuna.log());
}

/// And for Qt on X11, through the seat's Xwayland at scale 1.5: Qt counts in its own logical
/// pixels, X11's divided by its device pixel ratio, 1.5, which are Yuna's logical units.
#[test]
fn an_agent_finds_and_presses_a_qt_button_on_x11_at_one_and_a_half() {
    let mut yuna = Yuna::start("a11y-x11");
    let created = yuna.call(
        "seat",
        json!({"action": "create", "width": 1920, "height": 1080, "scale": 1.5}),
    );
    let seat = created["structuredContent"]["seat"]
        .as_u64()
        .unwrap_or_else(|| panic!("{created}"));
    let env = pinned_environment(&yuna, &qt_variables("xcb"));
    let scene = accessible_scene();
    let qml = yuna.start_program(seat, &["qml", &scene], &env, true);
    let button = press_qt_button(&mut yuna, seat, &qml);
    let windows = yuna.call("windows", json!({"seat": seat}));
    assert_eq!(
        windows["structuredContent"]["windows"][0]["shell"],
        json!("x11"),
        "{windows}"
    );
    assert_eq!(button["rect"]["width"], json!(240), "{button}");
    let status = yuna.close_and_wait();
    assert!(status.success(), "{status}\n{}", yuna.log());
}

/// The wake-ups Yuna counted in its last `counters` line, at the `debug` level.
fn wakeups(log: &str) -> u64 {
    log.lines()
        .rev()
        .filter(|line| line.contains(" counters "))
        .find_map(|line| {
            line.split_whitespace()
                .find_map(|field| field.strip_prefix("wakeups="))
                .and_then(|count| count.parse().ok())
        })
        .unwrap_or(0)
}

/// The clock ticks a second of `/proc/<pid>/stat`'s CPU times: Linux's `USER_HZ`, 100 on every
/// architecture Yuna builds for.
const TICKS_A_SECOND: f64 = 100.0;

/// A busy program slowed to 50 % runs half of the time: sampled every 3 ms over 2 s, it is
/// stopped in 0.4 to 0.6 of the samples, and its CPU time is at most 0.6 of the time elapsed,
/// the tenth above a half for the timer's lag and the ticks' grain, never measured against a
/// free run, which another test's load would move. It is stopped and continued by Yuna's timer,
/// 20 times a second, and Yuna wakes for those alone: SIGCHLD does not wake it when its child
/// stops or continues (`SA_NOCLDSTOP`), which would double the wake-ups.
#[test]
fn a_slowed_program_runs_half_the_time_and_its_stops_wake_nothing() {
    let mut yuna = Yuna::launch("slow", None, &[("YUNA_LOG", "debug")]);
    let created = yuna.call("seat", json!({"action": "create"}));
    assert_eq!(created["structuredContent"]["seat"], json!(SEAT));
    let started = yuna.call(
        "spawn",
        json!({"action": "start", "seat": SEAT, "command": ["sh", "-c", "while :; do :; done"]}),
    );
    let id = program_id(&started);
    let pid = started["structuredContent"]["program"]["pid"]
        .as_i64()
        .expect("a pid") as i32;
    let slowed = yuna.call(
        "spawn",
        json!({"action": "slow", "seat": SEAT, "program": id, "run_percent": 50}),
    );
    std::thread::sleep(Duration::from_millis(100));
    let woke_before = wakeups(&yuna.log());
    let ticks_before = cpu_ticks(pid);
    let began = Instant::now();
    let mut stopped_samples = 0;
    let mut samples = 0;
    while began.elapsed() < Duration::from_secs(2) {
        samples += 1;
        stopped_samples += u32::from(state_of(pid) == Some('T'));
        std::thread::sleep(Duration::from_millis(3));
    }
    let ticks = cpu_ticks(pid) - ticks_before;
    let woke = wakeups(&yuna.log()) - woke_before;
    let seconds = began.elapsed().as_secs_f64();
    let stopped = f64::from(stopped_samples) / f64::from(samples);
    let cpu = ticks as f64 / TICKS_A_SECOND / seconds;
    let per_second = woke as f64 / seconds;
    eprintln!(
        "slowed: stopped in {stopped:.2} of {samples} samples; CPU {cpu:.2} of the time \
         elapsed, {ticks} ticks in {seconds:.2} s; {per_second:.1} wake-ups a second"
    );
    let thawed = yuna.call(
        "spawn",
        json!({"action": "thaw", "seat": SEAT, "program": id}),
    );
    let refused: Vec<Value> = [0, 100]
        .into_iter()
        .map(|wrong| {
            let call = json!({"action": "slow", "seat": SEAT, "program": id, "run_percent": wrong});
            yuna.call("spawn", call)["isError"].clone()
        })
        .collect();
    // The busy program ends before any assertion: a failed one drops `Yuna`, which kills it with
    // SIGKILL, and a Yuna killed so leaves its programs running.
    yuna.call(
        "spawn",
        json!({"action": "stop", "seat": SEAT, "program": id, "force": true}),
    );
    wait_for("the busy program's end", || (!alive(pid)).then_some(()));
    assert_eq!(
        slowed["structuredContent"]["program"]["run_percent"],
        json!(50),
        "{slowed}"
    );
    assert!((0.4..=0.6).contains(&stopped), "{stopped}");
    assert!(ticks > 0, "the slowed program never ran");
    assert!(cpu <= 0.6, "{cpu}");
    // 20 a second from the timer; as many again if SIGCHLD woke Yuna at each stop and continue.
    assert!(
        (5.0..=28.0).contains(&per_second),
        "{per_second} wake-ups a second"
    );
    assert_eq!(
        thawed["structuredContent"]["program"]["run_percent"],
        json!(100)
    );
    assert_eq!(refused, [json!(true), json!(true)]);
    let status = yuna.close_and_wait();
    assert!(status.success(), "{status}\n{}", yuna.log());
}

/// A frozen program ends at SIGTERM's time, not SIGKILL's, 2 s later: every path that sends
/// SIGTERM sends SIGCONT after it. `spawn stop`, which sends no SIGKILL, ends it at once; so do
/// a seat's closing and Yuna's stopping.
#[test]
fn a_frozen_program_ends_at_sigterm() {
    use yuna::mcp::END_GRACE;
    let mut yuna = Yuna::start("frozen-end");
    let other = yuna.call("seat", json!({"action": "create"}));
    let other = other["structuredContent"]["seat"].clone();
    let mut frozen = Vec::new();
    for seat in [json!(SEAT), json!(SEAT), other.clone()] {
        let started = yuna.call(
            "spawn",
            json!({"action": "start", "seat": seat, "command": ["sleep", "600"]}),
        );
        let id = program_id(&started);
        let pid = started["structuredContent"]["program"]["pid"]
            .as_i64()
            .expect("a pid") as i32;
        let call = json!({"action": "freeze", "seat": seat, "program": id});
        assert_eq!(yuna.call("spawn", call)["isError"], json!(false));
        wait_for("its stop", || (state_of(pid) == Some('T')).then_some(()));
        frozen.push((id, pid));
    }
    // One frozen whose group is killed outright, not through Yuna, is no longer told frozen.
    let outright = yuna.call(
        "spawn",
        json!({"action": "start", "seat": SEAT, "command": ["sleep", "600"]}),
    );
    let outright_id = program_id(&outright);
    let outright_pid = outright["structuredContent"]["program"]["pid"]
        .as_i64()
        .expect("a pid") as i32;
    let call = json!({"action": "freeze", "seat": SEAT, "program": outright_id});
    assert_eq!(yuna.call("spawn", call)["isError"], json!(false));
    wait_for("its stop", || {
        (state_of(outright_pid) == Some('T')).then_some(())
    });
    kill_process(Pid::from_raw(outright_pid).expect("a pid"), Signal::KILL).expect("SIGKILL");
    let killed = wait_for("the killed program's end", || {
        let killed = program(&mut yuna, 2);
        (killed["state"] == json!("killed")).then_some(killed)
    });
    assert_eq!(killed["run_percent"], json!(100), "{killed}");

    let began = Instant::now();
    let stop = json!({"action": "stop", "seat": SEAT, "program": frozen[0].0});
    assert_eq!(yuna.call("spawn", stop)["isError"], json!(false));
    let ended = wait_for("the stopped program's end", || {
        let ended = program(&mut yuna, 0);
        (ended["state"] != json!("running")).then_some(ended)
    });
    assert_eq!(ended["signal"], json!(15), "{ended}");
    assert!(began.elapsed() < END_GRACE / 2, "{:?}", began.elapsed());

    let began = Instant::now();
    let closed = yuna.call("seat", json!({"action": "close", "seat": SEAT}));
    let took = began.elapsed();
    assert!(
        closed["structuredContent"]["ended"]
            .as_array()
            .is_some_and(|ended| ended.contains(&json!(frozen[1].1))),
        "{closed}"
    );
    assert!(took < END_GRACE / 2, "{took:?}");

    let began = Instant::now();
    let status = yuna.close_and_wait();
    let took = began.elapsed();
    assert!(status.success(), "{status}\n{}", yuna.log());
    assert!(!alive(frozen[2].1));
    // Its second for the last replies is not waited for: the output is read.
    assert!(took < END_GRACE, "{took:?}");
}

/// The names of the folders in `parent` that a Yuna makes before it gives them their names,
/// `.<prefix><pid>-<n>-starting`: none is left once its folder has its name.
fn staging_folders(parent: &std::path::Path, prefix: &str) -> Vec<String> {
    fs::read_dir(parent)
        .expect("the parent folder")
        .filter_map(|entry| entry.ok()?.file_name().into_string().ok())
        .filter(|name| name.starts_with(&format!(".{prefix}")))
        .collect()
}

/// A `yuna mcp` killed by SIGKILL leaves its folder in `XDG_RUNTIME_DIR`: the next one removes
/// it when it starts, when its lock file's lock is free, whatever pid its name holds, a retry's
/// `yuna-mcp-<pid>-<n>` too. It keeps a folder whose lock is held, though its pid is gone; one
/// with no lock file, an older Yuna's; a link to a folder, which it does not follow; and any
/// other name. A live Yuna holds its own folder's lock.
#[test]
fn an_old_yunas_folder_is_removed_at_start_and_a_living_ones_kept() {
    use yuna::mcp::LOCK_FILE;
    let top = test_folder("stale");
    let runtime = top.join("runtime");
    // Above any pid Linux gives (`pid_max` is 2²² at most), and the test's own, which lives:
    // neither decides.
    let gone = runtime.join("yuna-mcp-999999999");
    let gone_retry = runtime.join("yuna-mcp-999999998-1");
    let gone_living_pid = runtime.join(format!("yuna-mcp-{}", std::process::id()));
    let locked = runtime.join("yuna-mcp-999999997");
    let older = runtime.join("yuna-mcp-999999996");
    let elsewhere = top.join("elsewhere");
    let link = runtime.join("yuna-mcp-999999995");
    let unrelated = runtime.join("yuna-mcp-notes");
    for folder in [
        &gone,
        &gone_retry,
        &gone_living_pid,
        &locked,
        &older,
        &elsewhere,
        &unrelated,
    ] {
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(folder.join("logs"))
            .expect("a folder");
        fs::write(folder.join("logs/1.log"), "a log\n").expect("a log");
        if folder != &older {
            fs::write(folder.join(LOCK_FILE), "").expect("a lock file");
        }
    }
    std::os::unix::fs::symlink(&elsewhere, &link).expect("a link");
    let held = fs::File::open(locked.join(LOCK_FILE)).expect("the lock file");
    held.try_lock().expect("the lock, as a live Yuna holds it");

    let mut yuna = Yuna::start_in(top);
    assert_eq!(yuna.runtime_dir(), runtime);
    for removed in [&gone, &gone_retry, &gone_living_pid] {
        assert!(
            !removed.exists(),
            "{} is left\n{}",
            removed.display(),
            yuna.log()
        );
    }
    for kept in [&locked, &older, &unrelated] {
        assert!(kept.join("logs").exists(), "{} is gone", kept.display());
    }
    assert!(fs::symlink_metadata(&link).is_ok_and(|link| link.file_type().is_symlink()));
    assert!(elsewhere.join("logs/1.log").exists());
    let own = runtime.join(format!("yuna-mcp-{}", yuna.child.id()));
    let own_lock = fs::File::open(own.join(LOCK_FILE)).expect("its lock file");
    assert!(
        matches!(own_lock.try_lock(), Err(fs::TryLockError::WouldBlock)),
        "a live Yuna's lock is free"
    );
    assert_eq!(staging_folders(&runtime, "yuna-mcp-"), Vec::<String>::new());
    drop(held);
    let status = yuna.close_and_wait();
    assert!(status.success(), "{status}\n{}", yuna.log());
}

/// A seat's bus starts with its first program started with accessibility, never before; a
/// program started without gets the variables that keep its toolkit from looking for one, and
/// the tool says plainly there is none; with no application on it, a call answers at once; each
/// seat's programs get their own seat's bus alone.
/// The daemon killed, Yuna and the seat go on, its socket is removed, and the next program with
/// accessibility starts another; the seat closed, its daemon is ended among its processes.
#[test]
fn a_seats_bus_starts_on_demand_is_its_own_and_ends_with_its_seat() {
    let mut yuna = Yuna::start("a11y-bus");
    let yuna_pid = yuna.child.id() as i32;
    let env = BTreeMap::new();
    let print = ["sh", "-c", "env; sleep 600"];
    let plain = yuna.start_program(SEAT, &print, &env, false);
    let plain_log = PathBuf::from(plain["log"].as_str().expect("a log"));
    assert_eq!(
        printed_variable(&plain_log, "NO_AT_BRIDGE").as_deref(),
        Some("1")
    );
    assert_eq!(
        printed_variable(&plain_log, "GTK_A11Y").as_deref(),
        Some("none")
    );
    assert_eq!(printed_variable(&plain_log, "AT_SPI_BUS_ADDRESS"), None);
    assert!(
        daemons_of(yuna_pid).is_empty(),
        "a bus before any accessible program"
    );
    let folder = yuna
        .runtime_dir()
        .join(format!("yuna-mcp-{yuna_pid}"))
        .join("seat-2.a11y");
    assert!(!folder.exists(), "{}", folder.display());
    let none = yuna.call("accessibility", json!({"seat": SEAT}));
    assert_eq!(none["isError"], json!(true), "{none}");
    assert!(
        none["content"][0]["text"]
            .as_str()
            .is_some_and(|text| text.contains("seat 2 has no accessibility bus")),
        "{none}"
    );

    let accessible = yuna.start_program(SEAT, &print, &env, true);
    let log = PathBuf::from(accessible["log"].as_str().expect("a log"));
    let address = printed_variable(&log, "AT_SPI_BUS_ADDRESS").expect("its seat's bus");
    assert_eq!(
        address,
        format!("unix:path={}", folder.join("bus").display())
    );
    for (name, value) in yuna::mcp::ACCESSIBILITY_VARIABLES {
        assert_eq!(
            printed_variable(&log, name).as_deref(),
            Some(*value),
            "{name}"
        );
    }
    assert_eq!(printed_variable(&log, "NO_AT_BRIDGE"), None);
    let first = daemons_of(yuna_pid);
    assert_eq!(first.len(), 1, "{first:?}");
    assert!(folder.join("bus").exists());
    // An empty name, which would match every node, is refused.
    let empty_name = yuna.call("accessibility", json!({"seat": SEAT, "name": ""}));
    assert_eq!(empty_name["isError"], json!(true), "{empty_name}");
    // A shell embeds no tree: the bus has no application, and a call says so at once.
    let began = Instant::now();
    let empty = yuna.accessible_nodes(json!({"seat": SEAT}));
    assert!(
        began.elapsed() < Duration::from_secs(1),
        "{:?}",
        began.elapsed()
    );
    assert_eq!(
        (&empty["nodes"], &empty["apps"]),
        (&json!([]), &json!([])),
        "{empty}"
    );

    // Another seat's programs get its own bus, never seat 2's.
    let other = yuna.call("seat", json!({"action": "create"}));
    let other = other["structuredContent"]["seat"].as_u64().expect("a seat");
    let neighbour = yuna.start_program(other, &print, &env, true);
    let neighbour_log = PathBuf::from(neighbour["log"].as_str().expect("a log"));
    let neighbour_address = printed_variable(&neighbour_log, "AT_SPI_BUS_ADDRESS").expect("a bus");
    assert_ne!(neighbour_address, address);
    assert!(
        neighbour_address.ends_with(&format!("seat-{other}.a11y/bus")),
        "{neighbour_address}"
    );
    let neighbour_plain = yuna.start_program(other, &print, &env, false);
    let neighbour_plain_log = PathBuf::from(neighbour_plain["log"].as_str().expect("a log"));
    assert_eq!(
        printed_variable(&neighbour_plain_log, "AT_SPI_BUS_ADDRESS"),
        None
    );
    assert!(
        !fs::read_to_string(&neighbour_plain_log)
            .unwrap_or_default()
            .contains("seat-2.a11y"),
        "seat 2's bus reached seat {other}'s program"
    );
    assert_eq!(daemons_of(yuna_pid).len(), 2);

    // The daemon killed: the seat goes on, its socket goes, and the next program starts another.
    kill_process(Pid::from_raw(first[0]).expect("a pid"), Signal::KILL).expect("SIGKILL is sent");
    wait_for("the dead bus's socket removed", || {
        (!folder.join("bus").exists()).then_some(())
    });
    let listed = yuna.call("seat", json!({"action": "list"}));
    assert_eq!(
        listed["structuredContent"]["seats"][0]["seat"],
        json!(SEAT),
        "{listed}"
    );
    let again = yuna.start_program(SEAT, &print, &env, true);
    let again_log = PathBuf::from(again["log"].as_str().expect("a log"));
    assert_eq!(
        printed_variable(&again_log, "AT_SPI_BUS_ADDRESS"),
        Some(address)
    );
    let second = wait_for("the new bus", || {
        let daemons: Vec<i32> = daemons_of(yuna_pid)
            .into_iter()
            .filter(|pid| *pid != first[0])
            .collect();
        (daemons.len() == 2).then_some(daemons)
    });
    assert!(folder.join("bus").exists());

    // The seat closed, its daemon is ended among its processes; the other seat's runs on.
    let closed = yuna.call("seat", json!({"action": "close", "seat": SEAT}));
    let ended = closed["structuredContent"]["ended"].clone();
    let seat_daemon = second
        .iter()
        .copied()
        .find(|pid| {
            ended
                .as_array()
                .is_some_and(|ended| ended.contains(&json!(pid)))
        })
        .unwrap_or_else(|| panic!("no daemon among {ended}: {second:?}"));
    assert!(!alive(seat_daemon));
    assert!(!folder.exists(), "{}", folder.display());
    assert_eq!(daemons_of(yuna_pid).len(), 1);
    // The other seat's daemon ends with Yuna: by its pid, since one that outlived Yuna would
    // have another parent.
    let other_daemon = second
        .iter()
        .copied()
        .find(|pid| *pid != seat_daemon)
        .expect("the other seat's daemon");
    assert!(alive(other_daemon));
    let status = yuna.close_and_wait();
    assert!(status.success(), "{status}\n{}", yuna.log());
    wait_for("the other seat's daemon's end", || {
        (!alive(other_daemon)).then_some(())
    });
}

/// A program that stops answering, stopped by SIGSTOP, costs a call its deadline and no more, is
/// told as such, and never keeps another program's tree from being read.
#[test]
fn a_program_that_stops_answering_costs_its_deadline_alone() {
    let mut yuna = Yuna::start("a11y-stopped");
    let env = pinned_environment(&yuna, &gtk_variables());
    let demo = yuna.start_program(SEAT, &["gtk4-demo", "--run=dialog"], &env, true);
    let qt_env = pinned_environment(&yuna, &qt_variables("wayland"));
    let scene = accessible_scene();
    let qml = yuna.start_program(SEAT, &["qml", &scene], &qt_env, true);
    for (program, name) in [(&demo, "message dialog"), (&qml, "Press me")] {
        yuna.accessible_nodes(json!({
            "seat": SEAT, "pid": program["pid"], "name": name, "wait_ms": 40_000
        }));
    }
    let stopped = Pid::from_raw(demo["pid"].as_i64().expect("a pid") as i32).expect("a pid");
    kill_process(stopped, Signal::STOP).expect("SIGSTOP is sent");
    // Qt's own walk must end within its deadline too, which a loaded machine may keep it from:
    // an attempt in which Qt did not answer in time is made again, up to three, each one checked
    // whole.
    let mut attempts = 0;
    let found = loop {
        attempts += 1;
        let began = Instant::now();
        let found = yuna.accessible_nodes(json!({"seat": SEAT, "role": "button"}));
        let took = began.elapsed();
        eprintln!(
            "attempt {attempts}: the call with a stopped program took {} ms",
            took.as_millis()
        );
        assert!(
            took >= yuna::mcp::APP_DEADLINE
                && took < yuna::mcp::APP_DEADLINE + Duration::from_secs(2),
            "{took:?}"
        );
        let status_of = |pid: &Value| {
            found["apps"]
                .as_array()
                .into_iter()
                .flatten()
                .find(|app| &app["pid"] == pid)
                .map(|app| app["status"].clone())
                .unwrap_or_else(|| panic!("no application {pid} in {found}"))
        };
        assert_eq!(
            status_of(&demo["pid"]),
            json!("no answer within 3 s"),
            "{found}"
        );
        if status_of(&qml["pid"]) == json!("complete") || attempts == 3 {
            assert_eq!(status_of(&qml["pid"]), json!("complete"), "{found}");
            break found;
        }
    };
    kill_process(stopped, Signal::CONT).expect("SIGCONT is sent");
    assert!(
        found["nodes"]
            .as_array()
            .is_some_and(|nodes| nodes.iter().any(|node| node["name"] == json!("Press me"))),
        "{found}"
    );
    let status = yuna.close_and_wait();
    assert!(status.success(), "{status}\n{}", yuna.log());
}

/// The context switches of every thread of process `pid`, voluntary and not. Panics when no
/// thread could be read, the process gone or `schedstat` missing: a reading of none would make
/// any two readings equal.
fn context_switches(pid: i32) -> u64 {
    let pid = u32::try_from(pid).expect("a process id");
    let threads = Proc::system().threads(pid);
    assert!(!threads.is_empty(), "the threads of process {pid}");
    threads
        .iter()
        .map(|thread| thread.voluntary + thread.involuntary)
        .sum()
}

/// How long the rest is measured: 30 s.
const ACCESSIBLE_REST: Duration = Duration::from_secs(30);

/// How long Yuna and the bus must go without a context switch before the rest is measured:
/// longer than the bus's reply timeout, 5 s. dbus-daemon arms a timer for the calls it forwards
/// that wait for a reply; a reply that comes removes its call but leaves the timer armed
/// (dbus 1.16.2, `bus/expirelist.c`), so that it fires once, up to 5 s after the walk's last
/// call, finds nothing to expire and stops.
const SETTLED: Duration = Duration::from_secs(6);

/// At rest, once a tree was read, nothing runs in Yuna or in the seat's bus: once both have
/// settled, over 30 s with an accessible GTK 4 program open, neither Yuna's threads nor the
/// daemon switch context once.
#[test]
fn yuna_and_the_bus_rest_once_a_tree_is_read() {
    let mut yuna = Yuna::start("a11y-rest");
    let env = pinned_environment(&yuna, &gtk_variables());
    yuna.start_program(SEAT, &["gtk4-demo", "--run=dialog"], &env, true);
    let found = yuna.accessible_nodes(json!({
        "seat": SEAT, "role": "button", "name": "message dialog", "wait_ms": 40_000
    }));
    let walked = Instant::now();
    let visited: u64 = found["apps"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|app| app["nodes_visited"].as_u64())
        .sum();
    let yuna_pid = yuna.child.id() as i32;
    let daemon = daemons_of(yuna_pid)[0];
    // Settled first: the walk's replies, the program's own start and the bus's last timer
    // behind.
    let settled_by = Instant::now() + Duration::from_secs(90);
    let mut last = (context_switches(yuna_pid), context_switches(daemon));
    let mut quiet_since = Instant::now();
    while quiet_since.elapsed() < SETTLED {
        std::thread::sleep(Duration::from_millis(500));
        let now = (context_switches(yuna_pid), context_switches(daemon));
        if now != last {
            last = now;
            quiet_since = Instant::now();
        }
        assert!(
            Instant::now() < settled_by,
            "Yuna and the bus never settled"
        );
    }
    eprintln!(
        "settled {} ms after the walk, Yuna and the bus without a context switch for {} s",
        walked.elapsed().as_millis() - SETTLED.as_millis(),
        SETTLED.as_secs()
    );
    let before = last;
    std::thread::sleep(ACCESSIBLE_REST);
    let after = (context_switches(yuna_pid), context_switches(daemon));
    eprintln!(
        "over {} s at rest after a walk of {visited} nodes: Yuna {} context switches ({} -> {}), \
         dbus-daemon {} ({} -> {})",
        ACCESSIBLE_REST.as_secs(),
        after.0 - before.0,
        before.0,
        after.0,
        after.1 - before.1,
        before.1,
        after.1
    );
    assert_eq!(after.0, before.0, "Yuna woke at rest");
    assert_eq!(after.1, before.1, "the bus woke at rest");
    let status = yuna.close_and_wait();
    assert!(status.success(), "{status}\n{}", yuna.log());
}

/// A daemon that never prints its address, named by `YUNA_DBUS_DAEMON`, is ended at the start's
/// limit, 5 s, and the program that waited for it is told so; what it wrote, 2 MiB, is kept in its
/// log as a program's output is, its first 256 KiB and its newest 1 MiB; a name that is no
/// executable file is refused at once.
/// Yuna and its seat go on.
#[test]
fn a_bus_that_never_starts_fails_its_program_at_the_limit() {
    let scripts = fresh_folder("fake-daemon");
    let silent = scripts.join("dbus-daemon");
    fs::write(
        &silent,
        format!(
            "#!{}\nhead -c 2097152 /dev/zero\nexec sleep 600\n",
            in_path("sh").display()
        ),
    )
    .expect("a script");
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(&silent, fs::Permissions::from_mode(0o755)).expect("executable");
    let print = ["sh", "-c", "sleep 600"];
    let env: BTreeMap<String, String> = BTreeMap::new();

    let silent_path = silent.display().to_string();
    let mut yuna = Yuna::launch(
        "a11y-silent",
        None,
        &[("YUNA_DBUS_DAEMON", silent_path.as_str())],
    );
    yuna.call("seat", json!({"action": "create"}));
    let began = Instant::now();
    let refused = yuna.call(
        "spawn",
        json!({"action": "start", "seat": SEAT, "command": print, "env": env, "accessibility": true}),
    );
    let took = began.elapsed();
    let text = refused["content"][0]["text"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    assert_eq!(refused["isError"], json!(true), "{refused}");
    assert!(text.contains("was not ready in 5 s"), "{text}");
    assert!(
        took >= Duration::from_secs(5) && took < Duration::from_secs(8),
        "{took:?}"
    );
    let listed = yuna.call("seat", json!({"action": "list"}));
    assert_eq!(
        listed["structuredContent"]["seats"][0]["seat"],
        json!(SEAT),
        "{listed}"
    );
    let log = yuna
        .runtime_dir()
        .join(format!("yuna-mcp-{}", yuna.child.id()))
        .join("seat-2.a11y/bus.log");
    let kept = wait_for("the daemon's log", || {
        let size = fs::metadata(&log).ok()?.len();
        (size == yuna::mcp::LOG_HEAD + yuna::mcp::LOG_TAIL).then_some(size)
    });
    assert_eq!(kept, (256 << 10) + (1 << 20));
    let status = yuna.close_and_wait();
    assert!(status.success(), "{status}\n{}", yuna.log());

    let mut yuna = Yuna::launch(
        "a11y-nowhere",
        None,
        &[("YUNA_DBUS_DAEMON", "/nonexistent/dbus-daemon")],
    );
    yuna.call("seat", json!({"action": "create"}));
    let refused = yuna.call(
        "spawn",
        json!({"action": "start", "seat": SEAT, "command": print, "env": env, "accessibility": true}),
    );
    assert_eq!(refused["isError"], json!(true), "{refused}");
    assert!(
        refused["content"][0]["text"]
            .as_str()
            .is_some_and(|text| text.contains("not an absolute path to an executable file")),
        "{refused}"
    );
    let status = yuna.close_and_wait();
    assert!(status.success(), "{status}\n{}", yuna.log());
    let _ = fs::remove_dir_all(&scripts);
}

/// The windows of seat [`SEAT`], as `windows` lists them.
fn windows_of(yuna: &mut Yuna) -> Vec<Value> {
    let listed = yuna.call("windows", json!({"seat": SEAT}));
    assert_eq!(listed["isError"], json!(false), "{listed}");
    listed["structuredContent"]["windows"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// Whether `rect`, as `windows` gives it, lies inside `output`, as `outputs` gives it.
fn rect_inside(rect: &Value, output: &Value) -> bool {
    let (Some(x), Some(y), Some(width), Some(height)) = (
        rect["x"].as_f64(),
        rect["y"].as_f64(),
        rect["width"].as_f64(),
        rect["height"].as_f64(),
    ) else {
        return false;
    };
    let area = &output["logical"];
    let (Some(left), Some(top), Some(across), Some(down)) = (
        area["x"].as_f64(),
        area["y"].as_f64(),
        area["width"].as_f64(),
        area["height"].as_f64(),
    ) else {
        return false;
    };
    width > 0.0
        && height > 0.0
        && x >= left
        && y >= top
        && x + width <= left + across
        && y + height <= top + down
}

/// M1.11's done-when, as an agent lives it, in one `yuna mcp`, every step a tool call and
/// nothing read outside MCP's replies: the agent finds a program's protocol error and its log; it
/// finds a GTK 4 button by its name and presses it at its centre; and it checks a window across
/// an output unplugged, which `events` tells and `windows` and `capture` confirm; then it closes
/// its seat, and the call lists what it ended.
#[test]
fn an_agent_lives_m1_11s_done_when_through_mcp_alone() {
    let mut yuna = Yuna::start_without_seat("done-when");
    let created = yuna.call(
        "seat",
        json!({"action": "create", "width": 1280, "height": 720, "rate": 60, "scale": 1}),
    );
    assert_eq!(
        created["structuredContent"]["seat"],
        json!(SEAT),
        "{created}"
    );
    let second = yuna.call(
        "outputs",
        json!({"action": "create", "seat": SEAT, "width": 1280, "height": 720, "rate": 60,
               "scale": 1}),
    );
    assert_eq!(second["isError"], json!(false), "{second}");
    let second = second["structuredContent"]["output"]["id"].clone();
    let listed = yuna.call("outputs", json!({"action": "list", "seat": SEAT}));
    let outputs = listed["structuredContent"]["outputs"].clone();
    assert_eq!(outputs.as_array().map(Vec::len), Some(2), "{listed}");
    let first = outputs[0].clone();
    assert_ne!(first["id"], second, "{listed}");
    let second_output = outputs[1].clone();
    assert_eq!(second_output["id"], second, "{listed}");

    // A protocol error: told with its interface, code and message, kept on the program, and the
    // program's own log read.
    let start = events(&mut yuna, json!({"seat": SEAT}))["next"].clone();
    let broken = yuna.call(
        "spawn",
        json!({"action": "start", "seat": SEAT, "command": ["python3", "-I", "-c", ROLE_TWICE]}),
    );
    let broken_id = program_id(&broken);
    let told = events(
        &mut yuna,
        json!({"seat": SEAT, "after": start, "kinds": ["client_left"], "wait_ms": 20_000}),
    );
    let left = &told["events"][0];
    assert_eq!(left["reason"], json!("protocol_error"), "{told}");
    assert_eq!(left["program"], json!(broken_id), "{told}");
    assert_eq!(
        left["error"]["interface"],
        json!("wl_subcompositor"),
        "{told}"
    );
    // `wl_subcompositor.error.bad_surface`.
    assert_eq!(left["error"]["code"], json!(0), "{told}");
    assert!(
        left["error"]["message"]
            .as_str()
            .is_some_and(|text| !text.is_empty()),
        "{told}"
    );
    assert_eq!(
        program(&mut yuna, 0)["protocol_error"],
        left["error"],
        "{told}"
    );
    let log = wait_for("the broken program's log", || {
        let read = yuna.call(
            "spawn",
            json!({"action": "log", "seat": SEAT, "program": broken_id}),
        );
        assert_eq!(read["isError"], json!(false), "{read}");
        read["structuredContent"]["text"]
            .as_str()
            .filter(|text| text.contains("closed"))
            .map(str::to_owned)
    });
    assert_eq!(log, "closed\n");

    // A button, found by its name and pressed at its rectangle's centre; the dialog it opens is
    // read back, then closed by its own button.
    let env = pinned_environment(&yuna, &gtk_variables());
    let sleeper = yuna.start_program(SEAT, &["sleep", "600"], &env, false);
    let demo = yuna.start_program(SEAT, &["gtk4-demo", "--run=dialog"], &env, true);
    let found = yuna.accessible_nodes(json!({
        "seat": SEAT, "role": "button", "name": "message dialog", "wait_ms": 40_000
    }));
    let (button, x, y) = first_node_centre(&found, "the Message Dialog button");
    assert_eq!(button["app"], demo["pid"], "{found}");
    let window = button["window"].clone();
    let windows_before = windows_of(&mut yuna).len();
    let clicked = yuna.call(
        "pointer",
        json!({"action": "click", "seat": SEAT, "x": x, "y": y, "output": button["output"]}),
    );
    assert_eq!(
        clicked["structuredContent"]["pointer"]["window"], window,
        "{clicked}"
    );
    let dialog = yuna.accessible_nodes(json!({
        "seat": SEAT, "name": "Test message", "wait_ms": 30_000
    }));
    assert!(
        !dialog["nodes"].as_array().is_none_or(Vec::is_empty),
        "{dialog}"
    );
    wait_for("the dialog's window", || {
        (windows_of(&mut yuna).len() > windows_before).then_some(())
    });
    let cancel = yuna.accessible_nodes(json!({
        "seat": SEAT, "role": "button", "name": "cancel", "wait_ms": 30_000
    }));
    let (cancel_button, x, y) = first_node_centre(&cancel, "the dialog's Cancel button");
    yuna.call(
        "pointer",
        json!({"action": "click", "seat": SEAT, "x": x, "y": y, "output": cancel_button["output"]}),
    );
    wait_for("the dialog's end", || {
        (windows_of(&mut yuna).len() == windows_before).then_some(())
    });

    // The window, moved to the second output, then that output unplugged.
    let window_id = window.as_u64().expect("a window id");
    let away = yuna.call(
        "workspaces",
        json!({"action": "create", "seat": SEAT, "name": "away", "output": second}),
    );
    assert_eq!(away["isError"], json!(false), "{away}");
    let moved = yuna.call(
        "workspaces",
        json!({"action": "move_window", "seat": SEAT, "window": window, "workspace": "away"}),
    );
    assert_eq!(moved["isError"], json!(false), "{moved}");
    let shown = yuna.call(
        "workspaces",
        json!({"action": "focus", "seat": SEAT, "name": "away"}),
    );
    assert_eq!(shown["isError"], json!(false), "{shown}");
    let on_second = wait_for("the window on the second output", || {
        let window = windows_of(&mut yuna)
            .into_iter()
            .find(|candidate| candidate["id"] == json!(window_id))?;
        (window["output"] == second && window["rect"].is_object()).then_some(window)
    });
    assert!(
        rect_inside(&on_second["rect"], &second_output),
        "{on_second}"
    );

    let before_unplug = events(&mut yuna, json!({"seat": SEAT}))["next"].clone();
    let removed = yuna.call(
        "outputs",
        json!({"action": "remove", "seat": SEAT, "output": second}),
    );
    assert_eq!(removed["isError"], json!(false), "{removed}");
    let told = events(
        &mut yuna,
        json!({"seat": SEAT, "after": before_unplug, "kinds": ["output_removed"],
               "wait_ms": 20_000}),
    );
    assert_eq!(told["events"][0]["kind"], json!("output_removed"), "{told}");
    assert_eq!(told["events"][0]["output"], second, "{told}");
    // Its workspace went to the first output with its window, which a workspace the output does
    // not show hides: no output, no rectangle. The agent shows the workspace.
    let hidden = windows_of(&mut yuna)
        .into_iter()
        .find(|candidate| candidate["id"] == json!(window_id))
        .expect("the window is still there");
    assert_eq!(hidden["output"], Value::Null, "{hidden}");
    assert_eq!(hidden["workspace"], json!("away"), "{hidden}");
    let shown = yuna.call(
        "workspaces",
        json!({"action": "focus", "seat": SEAT, "name": "away"}),
    );
    assert_eq!(shown["isError"], json!(false), "{shown}");
    // The unplug told the window hidden, `window_placed` with no output; showing its workspace
    // tells it placed on the first output.
    let placed = wait_for("the window placed on the first output", || {
        let told = events(
            &mut yuna,
            json!({"seat": SEAT, "after": before_unplug, "kinds": ["window_placed"]}),
        );
        told["events"]
            .as_array()?
            .iter()
            .find(|event| event["window"] == window && event["output"] == first["id"])
            .cloned()
    });
    assert!(rect_inside(&placed["place"], &first), "{placed}");
    let on_first = windows_of(&mut yuna)
        .into_iter()
        .find(|candidate| candidate["id"] == json!(window_id))
        .expect("the window is still there");
    assert_eq!(on_first["output"], first["id"], "{on_first}");
    assert!(
        rect_inside(&on_first["rect"], &first),
        "{on_first}\n{first}"
    );
    let captured = yuna.call("capture", json!({"seat": SEAT, "window": window_id}));
    assert_eq!(captured["isError"], json!(false), "{captured}");
    assert!(
        captured["content"]
            .as_array()
            .is_some_and(|parts| parts.iter().any(|part| part["type"] == json!("image"))),
        "{captured}"
    );

    // The seat's closing ends its programs, and lists what it ended.
    let closed = yuna.call("seat", json!({"action": "close", "seat": SEAT}));
    assert_eq!(closed["isError"], json!(false), "{closed}");
    let ended = closed["structuredContent"]["ended"]
        .as_array()
        .unwrap_or_else(|| panic!("{closed}"));
    // A program that ignores its display's end, and the seat's accessibility bus's daemon.
    assert!(ended.contains(&sleeper["pid"]), "{closed}");
    assert!(ended.len() >= 2, "{closed}");
    assert_eq!(
        closed["structuredContent"]["left_running"],
        json!([]),
        "{closed}"
    );
    assert_eq!(
        closed["structuredContent"]["closed"],
        json!(SEAT),
        "{closed}"
    );
    let status = yuna.close_and_wait();
    assert!(status.success(), "{status}\n{}", yuna.log());
}

/// A Wayland client written by hand, in Python's standard library alone: a window of 64 × 64
/// pixels, application id `yuna.test.animated`, drawn from two `wl_shm` buffers in turn, each
/// frame committed when the last one's callback comes, for as many seconds as its argument says;
/// then, its last frame called back, it prints `still` and reads until its connection closes,
/// drawing nothing more.
const ANIMATED: &str = r#"
import os, socket, struct, sys, time

def message(target, opcode, payload=b""):
    return struct.pack("=II", target, (8 + len(payload)) << 16 | opcode) + payload

def string(text):
    data = text.encode() + b"\0"
    return struct.pack("=I", len(data)) + data + b"\0" * (-len(data) % 4)

wayland = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
wayland.connect(os.environ["WAYLAND_DISPLAY"])
data = b""

def events():
    """The events that came, each (object, opcode, body); a protocol error ends the client."""
    global data
    chunk = wayland.recv(65536)
    if not chunk:
        sys.exit("the connection closed")
    data += chunk
    found = []
    while len(data) >= 8:
        target, word = struct.unpack("=II", data[:8])
        size = word >> 16
        if len(data) < size:
            break
        found.append((target, word & 0xFFFF, data[8:size]))
        data = data[size:]
    for target, opcode, body in found:
        if target == 1 and opcode == 0:
            sys.exit("a protocol error: %r" % body)
    return found

# wl_display.get_registry, object 2, then wl_display.sync, object 3.
wayland.sendall(message(1, 1, struct.pack("=I", 2)) + message(1, 0, struct.pack("=I", 3)))
names, synced = {}, False
while not synced:
    for target, opcode, body in events():
        if target == 2 and opcode == 0:
            name, length = struct.unpack("=II", body[:8])
            names[body[8:8 + length - 1].decode()] = name
        synced = synced or target == 3

def bind(interface, version, new_id):
    return message(2, 0, struct.pack("=I", names[interface]) + string(interface)
                   + struct.pack("=II", version, new_id))

COMPOSITOR, SHM, WM_BASE, SURFACE, XDG_SURFACE, TOPLEVEL, POOL = 4, 5, 6, 7, 8, 9, 10
BUFFERS = (11, 12)
SIDE = 64
SIZE = SIDE * SIDE * 4
requests = bind("wl_compositor", 4, COMPOSITOR) + bind("wl_shm", 1, SHM)
requests += bind("xdg_wm_base", 1, WM_BASE)
requests += message(COMPOSITOR, 0, struct.pack("=I", SURFACE))
requests += message(WM_BASE, 2, struct.pack("=II", XDG_SURFACE, SURFACE))
requests += message(XDG_SURFACE, 1, struct.pack("=I", TOPLEVEL))
requests += message(TOPLEVEL, 3, string("yuna.test.animated"))
requests += message(SURFACE, 6)
wayland.sendall(requests)
memory = os.memfd_create("buffers")
os.ftruncate(memory, 2 * SIZE)
os.pwrite(memory, b"\x40\x80\xc0\xff" * (SIDE * SIDE) + b"\xc0\x80\x40\xff" * (SIDE * SIDE), 0)
# wl_shm.create_pool's descriptor travels beside its message.
socket.send_fds(wayland, [message(SHM, 0, struct.pack("=Ii", POOL, 2 * SIZE))], [memory])
for index, buffer in enumerate(BUFFERS):
    wayland.sendall(message(POOL, 0, struct.pack("=IiiiiI", buffer, index * SIZE, SIDE, SIDE,
                                                   SIDE * 4, 1)))

def acknowledge(found):
    """Acknowledges each xdg_surface.configure among the events found."""
    configured = False
    for target, opcode, body in found:
        if target == XDG_SURFACE and opcode == 0:
            wayland.sendall(message(XDG_SURFACE, 4, body[:4]))
            configured = True
    return configured

while not acknowledge(events()):
    pass
next_id = 13

def draw(frame):
    """Attaches the frame's buffer, damaged whole, asks its callback, commits; the callback's id."""
    global next_id
    callback, next_id = next_id, next_id + 1
    request = message(SURFACE, 1, struct.pack("=Iii", BUFFERS[frame % 2], 0, 0))
    request += message(SURFACE, 9, struct.pack("=iiii", 0, 0, SIDE, SIDE))
    request += message(SURFACE, 3, struct.pack("=I", callback))
    request += message(SURFACE, 6)
    wayland.sendall(request)
    return callback

until = time.monotonic() + float(sys.argv[1])
waiting, drawn = draw(0), 1
while time.monotonic() < until:
    found = events()
    acknowledge(found)
    if any(target == waiting and opcode == 0 for target, opcode, _ in found):
        waiting, drawn = draw(drawn), drawn + 1
while not any(target == waiting and opcode == 0 for target, opcode, _ in events()):
    pass
print("still", flush=True)
while True:
    acknowledge(events())
"#;

/// The `rest` result of a call with `arguments`.
fn rest(yuna: &mut Yuna, arguments: Value) -> Value {
    let result = yuna.call("rest", arguments);
    assert_eq!(result["isError"], json!(false), "{result}");
    result["structuredContent"].clone()
}

/// Runs a call that wakes Yuna until nothing is armed: the seat's first frame drawn, and what
/// follows it.
fn settle(yuna: &mut Yuna) {
    wait_for("Yuna's rest", || {
        let clock = yuna.call("clock", json!({"action": "now"}));
        clock["structuredContent"]["next_deadline_ns"]
            .is_null()
            .then_some(())
    });
}

/// Starts [`ANIMATED`] on [`SEAT`] for `seconds`, and waits for its window.
fn start_animated(yuna: &mut Yuna, seconds: u32) -> Value {
    let script = yuna.folder.join("animated.py");
    fs::write(&script, ANIMATED).expect("the client's script");
    let started = yuna.call(
        "spawn",
        json!({"action": "start", "seat": SEAT, "command": [
            "python3", "-I", script, seconds.to_string()
        ]}),
    );
    assert_eq!(started["isError"], json!(false), "{started}");
    let found = yuna.call(
        "windows",
        json!({"seat": SEAT, "app_id": "yuna.test.animated", "wait_ms": 20_000}),
    );
    let log = started["structuredContent"]["program"]["log"]
        .as_str()
        .map(|log| fs::read_to_string(log).unwrap_or_default())
        .unwrap_or_default();
    assert_eq!(
        found["structuredContent"]["windows"][0]["app_id"],
        json!("yuna.test.animated"),
        "{found}\n{log}\n{}",
        yuna.log()
    );
    started
}

/// The entry of `list` whose `key` is `value`.
fn entry<'a>(list: &'a Value, key: &str, value: &Value) -> Option<&'a Value> {
    list.as_array()?.iter().find(|entry| &entry[key] == value)
}

/// `yuna mcp` at rest, its seat's first frame drawn: a second of `rest` names nothing that woke
/// it and nothing drawn, its seat's or any; its own timer, and the wake that timer caused alone,
/// are left out.
#[test]
fn at_rest_a_second_of_yuna_mcp_names_nothing() {
    let mut yuna = Yuna::start("rest-quiet");
    settle(&mut yuna);
    let nothing = json!({
        "wakes": 0, "sources": [], "clients": [], "unnamed_client_wakes": 0, "programs": [],
        "clients_gone": {"clients": 0, "wakes": 0, "commits": 0, "draws": 0},
        "frames": 0, "draw_causes": [],
    });
    for arguments in [
        json!({"seat": SEAT, "wait_ms": 1000}),
        json!({"wait_ms": 1000}),
    ] {
        let mut rested = rest(&mut yuna, arguments.clone());
        for key in ["seat", "since_ns", "now_ns", "wait_ms"] {
            rested.as_object_mut().expect("an object").remove(key);
        }
        assert_eq!(rested, nothing, "{arguments}\n{}", yuna.log());
    }
    let status = yuna.close_and_wait();
    assert!(status.success(), "{status}\n{}", yuna.log());
}

/// With `--rest-audit` on, a second of `rest` at rest still names nothing: the log's own timer,
/// which the call's request armed, is never counted, in the window nor among Yuna's wakes.
#[test]
fn at_rest_with_the_rest_audit_on_a_second_names_nothing() {
    let mut yuna = Yuna::launch_with_options("rest-quiet-audit", None, &[], &["--rest-audit"]);
    let created = yuna.call("seat", json!({"action": "create"}));
    assert_eq!(created["structuredContent"]["seat"], json!(SEAT));
    settle(&mut yuna);
    // Past the second the settling calls opened, so that this call's request opens the next.
    std::thread::sleep(Duration::from_millis(1500));
    let mut rested = rest(&mut yuna, json!({"wait_ms": 1500}));
    for key in ["seat", "since_ns", "now_ns", "wait_ms"] {
        rested.as_object_mut().expect("an object").remove(key);
    }
    let nothing = json!({
        "wakes": 0, "sources": [], "clients": [], "unnamed_client_wakes": 0, "programs": [],
        "clients_gone": {"clients": 0, "wakes": 0, "commits": 0, "draws": 0},
        "frames": 0, "draw_causes": [],
    });
    assert_eq!(rested, nothing, "{}", yuna.log());
    assert!(
        !rest_audit_lines(&yuna.log()).is_empty(),
        "the requests' seconds were not logged\n{}",
        yuna.log()
    );
    let status = yuna.close_and_wait();
    assert!(status.success(), "{status}\n{}", yuna.log());
}

/// A program whose Wayland client draws as fast as its frames are called back is named by its
/// program, with its commits as the cause of the frames drawn; the clients' source counts its
/// wakes.
#[test]
fn a_program_that_animates_is_named_with_its_commits_as_the_frames_cause() {
    let mut yuna = Yuna::start("rest-animated");
    let started = start_animated(&mut yuna, 600);
    let id = json!(program_id(&started));
    let window = rest(&mut yuna, json!({"seat": SEAT, "wait_ms": 2000}));
    // The program ends before any assertion, which would leave it running.
    yuna.call(
        "spawn",
        json!({"action": "stop", "seat": SEAT, "program": id, "force": true}),
    );
    let client = entry(&window["clients"], "program", &id)
        .unwrap_or_else(|| panic!("the program is not named: {window}\n{}", yuna.log()))
        .clone();
    eprintln!("in 2 s: {client}");
    assert_eq!(client["seat"], json!(SEAT));
    assert_eq!(client["xwayland"], json!(false));
    assert_eq!(
        client["pid"],
        started["structuredContent"]["program"]["pid"]
    );
    let draws = client["draws"].as_u64().expect("draws");
    let commits = client["commits"].as_u64().expect("commits");
    // Its rate follows the machine's load; each commit is a new buffer, drawn once.
    assert!(commits >= 2 && draws >= 1, "{window}");
    assert!(commits <= draws + 1 && draws <= commits + 1, "{window}");
    let commit_frames = entry(&window["draw_causes"], "cause", &json!("commit"))
        .and_then(|cause| cause["frames"].as_u64());
    assert_eq!(commit_frames, Some(draws), "{window}");
    let clients = entry(&window["sources"], "source", &json!("clients"));
    assert_eq!(
        clients.and_then(|source| source["wakes"].as_u64()),
        client["wakes"].as_u64(),
        "{window}"
    );
    let status = yuna.close_and_wait();
    assert!(status.success(), "{status}\n{}", yuna.log());
}

/// A program that writes to its output is named as such: the programs' output among the
/// sources, and the program by its id, with its output's wakes.
#[test]
fn a_program_writing_to_its_output_is_named() {
    let mut yuna = Yuna::start("rest-output");
    // The seat's first frame drawn first, so that the window draws nothing.
    settle(&mut yuna);
    let started = yuna.call(
        "spawn",
        json!({"action": "start", "seat": SEAT, "command": [
            "sh", "-c", "while :; do echo tick; sleep 0.05; done"
        ]}),
    );
    let id = json!(program_id(&started));
    let window = rest(&mut yuna, json!({"seat": SEAT, "wait_ms": 1000}));
    yuna.call(
        "spawn",
        json!({"action": "stop", "seat": SEAT, "program": id, "force": true}),
    );
    let output = entry(&window["sources"], "source", &json!("program_output"))
        .and_then(|source| source["wakes"].as_u64())
        .unwrap_or(0);
    assert!(output >= 5, "{window}");
    let program = entry(&window["programs"], "program", &id)
        .unwrap_or_else(|| panic!("the program is not named: {window}"));
    assert_eq!(program["seat"], json!(SEAT));
    assert_eq!(program["output_wakes"].as_u64(), Some(output), "{window}");
    assert_eq!(program["pace_wakes"], json!(0));
    assert_eq!(window["frames"], json!(0), "{window}");
    let status = yuna.close_and_wait();
    assert!(status.success(), "{status}\n{}", yuna.log());
}

/// The lines of the rest audit in Yuna's log.
fn rest_audit_lines(log: &str) -> Vec<String> {
    log.lines()
        .filter(|line| line.contains("rest audit: what woke Yuna in one second"))
        .map(str::to_owned)
        .collect()
}

/// When a rest audit's line was logged, in seconds, from its timestamp,
/// `2026-11-18T04:21:25.983280Z`: within one month, which a test does not cross.
fn logged_at(line: &str) -> f64 {
    let stamp = line.split_whitespace().next().unwrap_or_default();
    let field = |range: std::ops::Range<usize>| -> f64 {
        stamp
            .get(range)
            .and_then(|text| text.parse().ok())
            .unwrap_or_else(|| panic!("no timestamp in {line:?}"))
    };
    let seconds = stamp
        .get(17..)
        .and_then(|text| text.trim_end_matches('Z').parse::<f64>().ok())
        .unwrap_or_else(|| panic!("no timestamp in {line:?}"));
    field(8..10) * 86_400.0 + field(11..13) * 3_600.0 + field(14..16) * 60.0 + seconds
}

/// With `--rest-audit`, Yuna logs one line for each second something woke it, a second apart
/// at least: while a program animates, its seconds name it, by program, with its commits and the
/// frames they caused; its last line names its output, `still` written once its last frame was
/// called back; then nothing, its window still open.
#[test]
fn the_rest_audit_logs_each_second_that_woke_yuna_and_nothing_at_rest() {
    let mut yuna = Yuna::launch_with_options("rest-audit", None, &[], &["--rest-audit"]);
    let created = yuna.call("seat", json!({"action": "create"}));
    assert_eq!(created["structuredContent"]["seat"], json!(SEAT));
    // 2 s, then still.
    let started = start_animated(&mut yuna, 2);
    let id = program_id(&started);
    // From here no call is made, which would wake Yuna. Its last wake is its output's, `still`;
    // the line of that second comes a second later, later still on a loaded machine.
    let lines = wait_for("the line naming the program's output", || {
        let lines = rest_audit_lines(&yuna.log());
        lines
            .iter()
            .any(|line| line.contains("program_output"))
            .then_some(lines)
    });
    std::thread::sleep(Duration::from_secs(3));
    let later = rest_audit_lines(&yuna.log());
    let named = format!("program {id} seat {SEAT}: ");
    let naming: Vec<&String> = lines.iter().filter(|line| line.contains(&named)).collect();
    yuna.call(
        "spawn",
        json!({"action": "stop", "seat": SEAT, "program": id, "force": true}),
    );
    eprintln!("{}", lines.join("\n"));
    // One line a second at most: a second begins at a wake after the last one ended, and ends
    // a second after it began.
    for pair in lines.windows(2) {
        let apart = logged_at(&pair[1]) - logged_at(&pair[0]);
        assert!(
            apart >= 0.95,
            "lines {apart:.3} s apart\n{}",
            lines.join("\n")
        );
    }
    assert!(naming.len() >= 2, "{}", lines.join("\n"));
    assert!(
        naming
            .iter()
            .any(|line| line.contains("commit ") && line.contains("clients ")),
        "{}",
        lines.join("\n")
    );
    assert_eq!(later, lines, "Yuna logged at rest\n{}", yuna.log());
    let status = yuna.close_and_wait();
    assert!(status.success(), "{status}\n{}", yuna.log());
}

/// A seat's own program, still running, whose environment names a seat that does not exist, is
/// still the seat's: Yuna started it on that seat and has not reaped it, so its pid is its own.
/// Closing the seat ends it, and says so (Stage 1's audit, m1.13-audit-mcp-3).
#[test]
fn closing_a_seat_ends_its_own_program_whatever_its_environment_names() {
    let mut yuna = Yuna::start("close-named");
    let started = yuna.call(
        "spawn",
        json!({"action": "start", "seat": SEAT, "command": [
            "sh", "-c", "echo shell=$$; export YUNA_SEAT=99; exec sleep 600"
        ]}),
    );
    let program = &started["structuredContent"]["program"];
    let pid = program["pid"]
        .as_i64()
        .unwrap_or_else(|| panic!("{started}")) as i32;
    let log = PathBuf::from(program["log"].as_str().expect("a log"));
    assert_eq!(printed(&log, "shell"), pid);
    wait_for("the exec of sleep, its environment naming seat 99", || {
        let environ = fs::read(format!("/proc/{pid}/environ")).ok()?;
        environ
            .split(|&byte| byte == 0)
            .any(|entry| entry == b"YUNA_SEAT=99")
            .then_some(())
    });
    let listed = yuna.call("spawn", json!({"action": "list", "seat": SEAT}));
    assert_eq!(
        listed["structuredContent"]["programs"][0]["state"],
        json!("running"),
        "{listed}"
    );
    let closed = yuna.call("seat", json!({"action": "close", "seat": SEAT}));
    // `close` answers once the processes it found are gone. What happened is read before the
    // process is ended by hand, so that a failure leaves nothing running.
    let outlived = alive(pid);
    let _ = kill_process(Pid::from_raw(pid).expect("a pid"), Signal::KILL);
    eprintln!("seat close answered: {closed}");
    assert!(
        !outlived,
        "seat {SEAT}'s own running program, pid {pid}, outlived its seat's closing: {closed}"
    );
    let status = yuna.close_and_wait();
    assert!(status.success(), "{status}\n{}", yuna.log());
}

/// A process left in the group of a seat's program that ended, adopted by Yuna, whose
/// environment names a seat that never existed, is still the seat's: only a seat that exists or
/// existed says a process is another seat's, so closing the seat ends it.
#[test]
fn closing_a_seat_ends_an_orphan_whose_environment_names_a_seat_that_never_was() {
    let mut yuna = Yuna::start("close-never");
    let started = yuna.call(
        "spawn",
        json!({"action": "start", "seat": SEAT, "command": [
            "sh", "-c", "(export YUNA_SEAT=99; exec sleep 600) & echo orphan=$!"
        ]}),
    );
    let log = PathBuf::from(
        started["structuredContent"]["program"]["log"]
            .as_str()
            .unwrap_or_else(|| panic!("{started}")),
    );
    let orphan = printed(&log, "orphan");
    let yuna_pid = yuna.child.id() as i32;
    wait_for(
        "the orphan's adoption, its environment naming seat 99",
        || {
            let environ = fs::read(format!("/proc/{orphan}/environ")).ok()?;
            let named = environ
                .split(|&byte| byte == 0)
                .any(|entry| entry == b"YUNA_SEAT=99");
            (named && parent_of(orphan) == Some(yuna_pid)).then_some(())
        },
    );
    let closed = yuna.call("seat", json!({"action": "close", "seat": SEAT}));
    let outlived = alive(orphan);
    let _ = kill_process(Pid::from_raw(orphan).expect("a pid"), Signal::KILL);
    assert!(
        !outlived,
        "the orphan outlived its seat's closing: {closed}"
    );
    assert!(
        closed["structuredContent"]["ended"]
            .as_array()
            .is_some_and(|ended| ended.contains(&json!(orphan))),
        "{closed}"
    );
    let status = yuna.close_and_wait();
    assert!(status.success(), "{status}\n{}", yuna.log());
}

/// Whether process `pid`'s environment names seat `seat` in `YUNA_SEAT`.
fn names_seat(pid: i32, seat: u64) -> bool {
    let wanted = format!("YUNA_SEAT={seat}");
    fs::read(format!("/proc/{pid}/environ")).is_ok_and(|environ| {
        environ
            .split(|&byte| byte == 0)
            .any(|entry| entry == wanted.as_bytes())
    })
}

/// A seat's own program, still running, and a child of it, whose environments name another seat
/// that exists, are still the seat's: lineage through the seat's running program decides first,
/// whatever the environment says, so that a program of seat 2 that sets `YUNA_SEAT=3` while seat
/// 3 exists cannot outlive seat 2's closing. Seat 3's own program runs on.
#[test]
fn closing_a_seat_ends_its_program_and_its_child_whose_environments_name_another_seat() {
    let mut yuna = Yuna::start("close-other");
    let other = yuna.call("seat", json!({"action": "create"}));
    assert_eq!(other["structuredContent"]["seat"], json!(3), "{other}");
    let neighbour = yuna.start_program(3, &["sleep", "600"], &BTreeMap::new(), false);
    let neighbour = neighbour["pid"].as_i64().expect("a pid") as i32;
    let program = yuna.start_program(
        SEAT,
        &[
            "sh",
            "-c",
            "(export YUNA_SEAT=3; exec sleep 600) & echo child=$!; \
             export YUNA_SEAT=3; exec sleep 600",
        ],
        &BTreeMap::new(),
        false,
    );
    let pid = program["pid"].as_i64().expect("a pid") as i32;
    let log = PathBuf::from(program["log"].as_str().expect("a log"));
    let child = printed(&log, "child");
    wait_for(
        "the program's and its child's exec of sleep, their environments naming seat 3",
        || (names_seat(pid, 3) && names_seat(child, 3)).then_some(()),
    );
    assert_eq!(parent_of(child), Some(pid));

    let closed = yuna.call("seat", json!({"action": "close", "seat": SEAT}));
    // What happened is read before the processes are ended by hand, so that a failure leaves
    // nothing running.
    let outlived: Vec<i32> = [pid, child].into_iter().filter(|&p| alive(p)).collect();
    for process in [pid, child] {
        let _ = kill_process(Pid::from_raw(process).expect("a pid"), Signal::KILL);
    }
    assert!(
        outlived.is_empty(),
        "seat {SEAT}'s program {pid} and its child {child}: {outlived:?} outlived the seat's \
         closing: {closed}"
    );
    let ended = &closed["structuredContent"]["ended"];
    assert!(
        ended
            .as_array()
            .is_some_and(|ended| ended.contains(&json!(pid)) && ended.contains(&json!(child))),
        "{closed}"
    );
    assert!(alive(neighbour), "seat 3's program ended with seat {SEAT}");
    let status = yuna.close_and_wait();
    assert!(status.success(), "{status}\n{}", yuna.log());
}

/// The variables of the user's home and folders that a test gives Yuna, which its programs must
/// not see (ADR-0105), and the system's folders, which they keep.
const USERS_FOLDERS: &[(&str, &str)] = &[
    ("HOME", "/nonexistent/user"),
    ("XDG_CONFIG_HOME", "/nonexistent/user/config"),
    ("XDG_DATA_HOME", "/nonexistent/user/data"),
    ("XDG_CACHE_HOME", "/nonexistent/user/cache"),
    ("XDG_STATE_HOME", "/nonexistent/user/state"),
    ("XDG_CONFIG_DIRS", "/nonexistent/etc/xdg"),
    ("XDG_DATA_DIRS", "/nonexistent/share"),
];

/// The four XDG base folders a seat's home holds.
const XDG_HOMES: [&str; 4] = [
    "XDG_CONFIG_HOME",
    "XDG_DATA_HOME",
    "XDG_CACHE_HOME",
    "XDG_STATE_HOME",
];

/// A folder's permissions, its type left out.
fn mode_of(folder: &std::path::Path) -> u32 {
    fs::metadata(folder)
        .unwrap_or_else(|error| panic!("{}: {error}", folder.display()))
        .permissions()
        .mode()
        & 0o777
}

/// A seat's programs share a throwaway home
/// ([ADR-0105](../../../docs/adr/0105-an-agents-programs-on-a-throwaway-home.md)): made in
/// Yuna's `TMPDIR` at the seat's first program, readable by the user alone, with `HOME` and the
/// four XDG base folders inside it, never the user's, which Yuna was given; a file one program
/// writes there another finds; `XDG_CONFIG_DIRS` and `XDG_DATA_DIRS` stay Yuna's; a variable the
/// agent names in `env` wins, the others staying in the home; each seat has its own. The seat's
/// home goes with its closing, and the folder of homes with Yuna.
#[test]
fn a_seats_programs_share_a_throwaway_home_that_goes_with_the_seat() {
    let mut yuna = Yuna::launch("home", None, USERS_FOLDERS);
    for seat in [SEAT, 3] {
        let created = yuna.call("seat", json!({"action": "create"}));
        assert_eq!(
            created["structuredContent"]["seat"],
            json!(seat),
            "{created}"
        );
    }
    assert!(
        !yuna.homes_folder().exists(),
        "the homes are made before any program"
    );
    let env = BTreeMap::new();
    let writer = yuna.start_program(
        SEAT,
        &[
            "sh",
            "-c",
            "mkdir -p \"$XDG_CONFIG_HOME/app\"; echo kept > \"$XDG_CONFIG_HOME/app/settings\"; \
             env; sleep 600",
        ],
        &env,
        false,
    );
    let writer_log = PathBuf::from(writer["log"].as_str().expect("a log"));
    let home = PathBuf::from(printed_variable(&writer_log, "HOME").expect("HOME"));
    assert!(
        home.starts_with(yuna.homes_folder()),
        "{} is not in the seat's throwaway home",
        home.display()
    );
    let mut inside = Vec::new();
    for name in XDG_HOMES {
        let folder = PathBuf::from(
            printed_variable(&writer_log, name).unwrap_or_else(|| panic!("no {name}")),
        );
        assert!(
            folder.starts_with(&home) && folder != home,
            "{name}={}",
            folder.display()
        );
        inside.push(folder);
    }
    inside.sort();
    inside.dedup();
    assert_eq!(inside.len(), XDG_HOMES.len(), "{inside:?}");
    assert_eq!(
        printed_variable(&writer_log, "XDG_CONFIG_DIRS").as_deref(),
        Some("/nonexistent/etc/xdg")
    );
    assert_eq!(
        printed_variable(&writer_log, "XDG_DATA_DIRS").as_deref(),
        Some("/nonexistent/share")
    );
    assert!(home.is_dir(), "{}", home.display());
    for folder in [yuna.homes_folder(), home.clone()] {
        assert_eq!(mode_of(&folder), 0o700, "{}", folder.display());
    }
    let config = PathBuf::from(printed_variable(&writer_log, "XDG_CONFIG_HOME").expect("set"));
    assert_eq!(
        fs::read_to_string(config.join("app/settings")).expect("the file the program wrote"),
        "kept\n"
    );

    // Another program of the seat finds it.
    let reader = yuna.start_program(
        SEAT,
        &[
            "sh",
            "-c",
            "echo found=$(cat \"$XDG_CONFIG_HOME/app/settings\"); env; sleep 600",
        ],
        &env,
        false,
    );
    let reader_log = PathBuf::from(reader["log"].as_str().expect("a log"));
    assert_eq!(
        printed_variable(&reader_log, "found").as_deref(),
        Some("kept")
    );
    assert_eq!(
        printed_variable(&reader_log, "HOME").as_deref(),
        home.to_str()
    );
    // One that names HOME gets its own, and the XDG folders still in the seat's home.
    let own = BTreeMap::from([("HOME".to_owned(), "/nonexistent/own".to_owned())]);
    let named = yuna.start_program(SEAT, &["sh", "-c", "env; sleep 600"], &own, false);
    let named_log = PathBuf::from(named["log"].as_str().expect("a log"));
    assert_eq!(
        printed_variable(&named_log, "HOME").as_deref(),
        Some("/nonexistent/own")
    );
    assert_eq!(
        printed_variable(&named_log, "XDG_CONFIG_HOME").as_deref(),
        config.to_str()
    );
    // Another seat's programs have another home.
    let neighbour = yuna.start_program(3, &["sh", "-c", "env; sleep 600"], &env, false);
    let neighbour_log = PathBuf::from(neighbour["log"].as_str().expect("a log"));
    let neighbour_home = PathBuf::from(printed_variable(&neighbour_log, "HOME").expect("HOME"));
    assert!(
        neighbour_home.starts_with(yuna.homes_folder()) && neighbour_home != home,
        "{}",
        neighbour_home.display()
    );

    let closed = yuna.call("seat", json!({"action": "close", "seat": SEAT}));
    assert_eq!(closed["isError"], json!(false), "{closed}");
    assert!(!home.exists(), "{} outlived its seat", home.display());
    assert!(neighbour_home.is_dir(), "{}", neighbour_home.display());
    let status = yuna.close_and_wait();
    assert!(status.success(), "{status}\n{}", yuna.log());
    assert!(
        !yuna.homes_folder().exists(),
        "{} outlived Yuna",
        yuna.homes_folder().display()
    );
}

/// A `yuna mcp` killed by SIGKILL leaves its seats' homes in `TMPDIR`: the next one removes
/// their folder when it starts, under its runtime folder's rule (ADR-0105): when its lock file's
/// lock is free, whatever pid its name holds, a retry's `yuna-mcp-homes-<pid>-<n>` too. It keeps
/// a folder whose lock is held; one with no lock file; a link, which it does not follow; and any
/// other name, a runtime folder's among them. A live Yuna holds its own homes' lock.
#[test]
fn an_old_yunas_homes_are_removed_at_start_and_a_living_ones_kept() {
    use yuna::mcp::LOCK_FILE;
    let top = test_folder("homes-stale");
    let temp = top.join("tmp");
    let gone = temp.join("yuna-mcp-homes-999999999");
    let gone_retry = temp.join("yuna-mcp-homes-999999998-1");
    let locked = temp.join("yuna-mcp-homes-999999997");
    let no_lock = temp.join("yuna-mcp-homes-999999996");
    let link = temp.join("yuna-mcp-homes-999999995");
    let runtime_folder = temp.join("yuna-mcp-999999994");
    let unrelated = temp.join("yuna-mcp-homes-notes");
    let elsewhere = top.join("elsewhere");
    for folder in [
        &gone,
        &gone_retry,
        &locked,
        &no_lock,
        &runtime_folder,
        &unrelated,
        &elsewhere,
    ] {
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(folder.join("seat-2/.config"))
            .expect("a folder");
        fs::write(folder.join("seat-2/.config/settings"), "kept\n").expect("a file");
        if folder != &no_lock {
            fs::write(folder.join(LOCK_FILE), "").expect("a lock file");
        }
    }
    std::os::unix::fs::symlink(&elsewhere, &link).expect("a link");
    let held = fs::File::open(locked.join(LOCK_FILE)).expect("the lock file");
    held.try_lock().expect("the lock, as a live Yuna holds it");

    let mut yuna = Yuna::start_in(top);
    assert_eq!(yuna.temp_dir(), temp);
    for removed in [&gone, &gone_retry] {
        assert!(
            !removed.exists(),
            "{} is left\n{}",
            removed.display(),
            yuna.log()
        );
    }
    for kept in [&locked, &no_lock, &runtime_folder, &unrelated, &elsewhere] {
        assert!(
            kept.join("seat-2/.config/settings").exists(),
            "{} is gone",
            kept.display()
        );
    }
    assert!(fs::symlink_metadata(&link).is_ok_and(|link| link.file_type().is_symlink()));
    yuna.start_program(SEAT, &["sleep", "600"], &BTreeMap::new(), false);
    let own_lock = fs::File::open(yuna.homes_folder().join(LOCK_FILE)).expect("its lock file");
    assert!(
        matches!(own_lock.try_lock(), Err(fs::TryLockError::WouldBlock)),
        "a live Yuna's lock is free"
    );
    assert_eq!(
        staging_folders(&temp, "yuna-mcp-homes-"),
        Vec::<String>::new()
    );
    drop(held);
    let status = yuna.close_and_wait();
    assert!(status.success(), "{status}\n{}", yuna.log());
}

/// A panic of Yuna's state removes its seats' homes as it removes its folder
/// ([ADR-0105](../../../docs/adr/0105-an-agents-programs-on-a-throwaway-home.md),
/// [ADR-0080](../../../docs/adr/0080-what-survives-a-failure-and-yunas-own-panics.md)).
#[cfg(debug_assertions)]
#[test]
fn a_panic_of_the_state_removes_the_seats_homes() {
    let mut yuna = Yuna::launch("panic-home", None, PANIC_ON_CLOCK);
    let created = yuna.call("seat", json!({"action": "create"}));
    assert_eq!(
        created["structuredContent"]["seat"],
        json!(SEAT),
        "{created}"
    );
    let program = yuna.start_program(
        SEAT,
        &["sh", "-c", "env; sleep 600"],
        &BTreeMap::new(),
        false,
    );
    let log = PathBuf::from(program["log"].as_str().expect("a log"));
    let home = PathBuf::from(printed_variable(&log, "HOME").expect("HOME"));
    assert!(
        home.is_dir() && home.starts_with(yuna.homes_folder()),
        "{}",
        home.display()
    );
    yuna.send("clock", json!({}));
    let status = yuna.wait_for_exit();
    assert_eq!(status.code(), Some(101), "{status}\n{}", yuna.log());
    assert!(
        !yuna.homes_folder().exists(),
        "{} outlived the panic",
        yuna.homes_folder().display()
    );
}

/// A panic follows the link to the seats' homes only into the temporary folder Yuna chose. A
/// seat's program can reach that link, in the programs' folder, which holds its runtime folder:
/// one points it at a folder of the user's elsewhere that bears the homes' folder's own name,
/// and the panic keeps that folder.
#[cfg(debug_assertions)]
#[test]
fn a_panic_never_follows_the_homes_link_out_of_yunas_temporary_folder() {
    use yuna::mcp::HOMES_LINK;
    let mut yuna = Yuna::launch("panic-link", None, PANIC_ON_CLOCK);
    let created = yuna.call("seat", json!({"action": "create"}));
    assert_eq!(
        created["structuredContent"]["seat"],
        json!(SEAT),
        "{created}"
    );
    let elsewhere = yuna
        .folder
        .join("elsewhere")
        .join(format!("yuna-mcp-homes-{}", yuna.child.id()));
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&elsewhere)
        .expect("a folder of the user's");
    fs::write(elsewhere.join("kept"), "the user's\n").expect("a file of the user's");
    let env = BTreeMap::from([
        ("ELSEWHERE".to_owned(), elsewhere.display().to_string()),
        ("LINK".to_owned(), HOMES_LINK.to_owned()),
    ]);
    let program = yuna.start_program(
        SEAT,
        &[
            "sh",
            "-c",
            "ln -sfn \"$ELSEWHERE\" \"$XDG_RUNTIME_DIR/../../$LINK\" && echo linked=$$; \
             exec sleep 600",
        ],
        &env,
        false,
    );
    let log = PathBuf::from(program["log"].as_str().expect("a log"));
    printed(&log, "linked");
    assert_eq!(
        fs::read_link(yuna.programs_folder().join(HOMES_LINK)).expect("the link"),
        elsewhere
    );
    yuna.send("clock", json!({}));
    let status = yuna.wait_for_exit();
    assert_eq!(status.code(), Some(101), "{status}\n{}", yuna.log());
    assert!(
        elsewhere.join("kept").exists(),
        "{} was removed through the link",
        elsewhere.display()
    );
}

/// Nor does a panic follow a link put at the homes' folder's own name: a seat's program may
/// remove the folder, put there a link to a folder of the user's, and point the programs'
/// folder's link at it with a final slash, which makes the system follow a link; the panic keeps
/// the user's folder.
#[cfg(debug_assertions)]
#[test]
fn a_panic_never_follows_a_link_put_at_the_homes_folders_name() {
    use yuna::mcp::HOMES_LINK;
    let mut yuna = Yuna::launch("panic-slash", None, PANIC_ON_CLOCK);
    let created = yuna.call("seat", json!({"action": "create"}));
    assert_eq!(
        created["structuredContent"]["seat"],
        json!(SEAT),
        "{created}"
    );
    let elsewhere = yuna.folder.join("elsewhere");
    fs::DirBuilder::new()
        .mode(0o700)
        .create(&elsewhere)
        .expect("a folder of the user's");
    fs::write(elsewhere.join("kept"), "the user's\n").expect("a file of the user's");
    let homes = yuna.homes_folder();
    let env = BTreeMap::from([
        ("ELSEWHERE".to_owned(), elsewhere.display().to_string()),
        ("HOMES".to_owned(), homes.display().to_string()),
        ("LINK".to_owned(), HOMES_LINK.to_owned()),
    ]);
    let program = yuna.start_program(
        SEAT,
        &[
            "sh",
            "-c",
            "rm -r \"$HOMES\" && ln -s \"$ELSEWHERE\" \"$HOMES\" && \
             ln -sfn \"$HOMES/\" \"$XDG_RUNTIME_DIR/../../$LINK\" && echo linked=$$; \
             exec sleep 600",
        ],
        &env,
        false,
    );
    let log = PathBuf::from(program["log"].as_str().expect("a log"));
    printed(&log, "linked");
    assert_eq!(
        fs::read_link(yuna.programs_folder().join(HOMES_LINK))
            .expect("the link")
            .into_os_string(),
        format!("{}/", homes.display()).as_str()
    );
    yuna.send("clock", json!({}));
    let status = yuna.wait_for_exit();
    assert_eq!(status.code(), Some(101), "{status}\n{}", yuna.log());
    assert!(
        elsewhere.join("kept").exists(),
        "{} was emptied through the link",
        elsewhere.display()
    );
}

/// The home program `log` printed, with `env`.
fn printed_home(log: &std::path::Path) -> PathBuf {
    PathBuf::from(printed_variable(log, "HOME").expect("HOME"))
}

/// Starts a program on [`SEAT`] that prints its environment; gives its home.
fn start_in_home(yuna: &mut Yuna) -> PathBuf {
    let program = yuna.start_program(
        SEAT,
        &["sh", "-c", "env; sleep 600"],
        &BTreeMap::new(),
        false,
    );
    printed_home(&PathBuf::from(program["log"].as_str().expect("a log")))
}

/// The folder of the seats' homes, removed while Yuna runs, as a cleaner of the temporary folder
/// such as systemd-tmpfiles' age rule may, is made again at the next program, under the same
/// rules: readable by the user alone, locked, and named by the programs' folder's link. A folder
/// another process put at its name is not Yuna's: the new one takes another name, no home is
/// made in the other, and Yuna removes its own alone.
#[test]
fn a_homes_folder_removed_while_yuna_runs_is_made_again() {
    use yuna::mcp::{HOMES_LINK, LOCK_FILE};
    let mut yuna = Yuna::start("homes-again");
    let first = start_in_home(&mut yuna);
    assert!(
        first.starts_with(yuna.homes_folder()),
        "{}",
        first.display()
    );
    fs::remove_dir_all(yuna.homes_folder()).expect("the homes' folder removed, as a cleaner does");

    let again = start_in_home(&mut yuna);
    assert_eq!(
        again, first,
        "the folder takes its name again, which is free"
    );
    for folder in [yuna.homes_folder(), again.clone()] {
        assert_eq!(mode_of(&folder), 0o700, "{}", folder.display());
    }
    let lock = fs::File::open(yuna.homes_folder().join(LOCK_FILE)).expect("its lock file");
    assert!(
        matches!(lock.try_lock(), Err(fs::TryLockError::WouldBlock)),
        "the folder made again is not locked"
    );
    let link = yuna.programs_folder().join(HOMES_LINK);
    assert_eq!(fs::read_link(&link).expect("the link"), yuna.homes_folder());

    // Removed again, and another's folder put at its name.
    fs::remove_dir_all(yuna.homes_folder()).expect("the homes' folder removed");
    let theirs = yuna.homes_folder();
    fs::DirBuilder::new()
        .mode(0o700)
        .create(&theirs)
        .expect("another's folder");
    fs::write(theirs.join("theirs"), "").expect("another's file");
    let moved = start_in_home(&mut yuna);
    let folder = moved.parent().expect("the homes' folder").to_owned();
    let own_name = format!("yuna-mcp-homes-{}-", yuna.child.id());
    assert!(
        folder.parent() == Some(yuna.temp_dir().as_path())
            && folder
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with(&own_name)),
        "{}",
        moved.display()
    );
    assert!(
        !theirs.join(format!("seat-{SEAT}")).exists(),
        "a home was made in another's folder"
    );
    assert_eq!(fs::read_link(&link).expect("the link"), folder);

    let closed = yuna.call("seat", json!({"action": "close", "seat": SEAT}));
    assert_eq!(closed["isError"], json!(false), "{closed}");
    assert!(!moved.exists(), "{} outlived its seat", moved.display());
    let status = yuna.close_and_wait();
    assert!(status.success(), "{status}\n{}", yuna.log());
    assert!(!folder.exists(), "{} outlived Yuna", folder.display());
    assert!(
        theirs.join("theirs").exists(),
        "another's folder was removed"
    );
}

/// In a temporary folder every user shares, another user may take, before Yuna's first program,
/// each name its homes' folder would have taken in turn: `yuna-mcp-homes-<pid>`, which holds
/// Yuna's pid, and the numbered ones after it. The folder then takes a name no one could
/// foresee, and the others are left as they were.
#[test]
fn a_homes_folder_whose_names_are_taken_takes_another() {
    let mut yuna = Yuna::start("homes-taken");
    let temp = yuna.temp_dir();
    let pid = yuna.child.id();
    let taken: Vec<PathBuf> = std::iter::once(format!("yuna-mcp-homes-{pid}"))
        .chain((1..100).map(|n| format!("yuna-mcp-homes-{pid}-{n}")))
        .map(|name| temp.join(name))
        .collect();
    for folder in &taken {
        fs::DirBuilder::new()
            .mode(0o700)
            .create(folder)
            .expect("a name taken");
    }
    let home = start_in_home(&mut yuna);
    let folder = home.parent().expect("the homes' folder").to_owned();
    assert!(
        folder.parent() == Some(temp.as_path()) && !taken.contains(&folder),
        "{}",
        home.display()
    );
    for other in &taken {
        assert!(
            fs::read_dir(other).is_ok_and(|mut entries| entries.next().is_none()),
            "{} was changed",
            other.display()
        );
    }
    let status = yuna.close_and_wait();
    assert!(status.success(), "{status}\n{}", yuna.log());
    assert!(!folder.exists(), "{} outlived Yuna", folder.display());
    assert!(taken.iter().all(|other| other.is_dir()));
}

/// In a temporary folder every user shares, another user may take the name under which a Yuna's
/// homes' folder was made before it got its own, which held Yuna's pid: a folder Yuna cannot
/// remove. The homes' folder is still made, and the other left as it was.
#[test]
fn a_homes_folder_is_made_whatever_folder_another_user_put_in_its_way() {
    let mut yuna = Yuna::start("homes-staging");
    let taken = yuna
        .temp_dir()
        .join(format!(".yuna-mcp-homes-{}-starting", yuna.child.id()));
    // Another user's folder in a sticky `/tmp` cannot be removed: here, its file is in a folder
    // the test may not write.
    let shut = taken.join("theirs");
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&shut)
        .expect("another's folder");
    fs::write(shut.join("file"), "").expect("another's file");
    fs::set_permissions(&shut, fs::Permissions::from_mode(0o500)).expect("shut");
    let started = yuna.call(
        "spawn",
        json!({"action": "start", "seat": SEAT, "command": ["sh", "-c", "env; sleep 600"]}),
    );
    // Opened again before anything is asserted, so that the test's folder can be removed.
    fs::set_permissions(&shut, fs::Permissions::from_mode(0o700)).expect("opened");
    assert_eq!(
        started["isError"],
        json!(false),
        "{started}\n{}",
        yuna.log()
    );
    let home = printed_home(&PathBuf::from(
        started["structuredContent"]["program"]["log"]
            .as_str()
            .expect("a log"),
    ));
    assert!(home.starts_with(yuna.homes_folder()), "{}", home.display());
    assert!(shut.join("file").exists(), "another's folder was changed");
    let status = yuna.close_and_wait();
    assert!(status.success(), "{status}\n{}", yuna.log());
}

/// A capture's encoder that panics fails its capture alone
/// ([ADR-0080](../../../docs/adr/0080-what-survives-a-failure-and-yunas-own-panics.md)): the
/// call is answered with `isError`, the encoder is no longer counted, and Yuna goes on. The
/// panics are asked through `YUNA_PANIC_IN_ENCODERS`, which a debug build alone has: the first
/// `MAX_ENCODERS` + 1 encoders panic, one more than a count that kept them would let start, so
/// that each of those captures fails as its encoder's, none refused for encoders running; the
/// next is encoded.
#[cfg(debug_assertions)]
#[test]
fn an_encoders_panic_fails_its_capture_alone() {
    use yuna::mcp::{MAX_ENCODERS, PANIC_IN_ENCODERS};
    let panics = (MAX_ENCODERS + 1).to_string();
    let mut yuna = Yuna::launch(
        "encoder-panic",
        None,
        &[
            (PANIC_IN_ENCODERS, panics.as_str()),
            ("RUST_BACKTRACE", "0"),
        ],
    );
    let created = yuna.call("seat", json!({"action": "create"}));
    assert_eq!(
        created["structuredContent"]["seat"],
        json!(SEAT),
        "{created}"
    );
    for _ in 0..=MAX_ENCODERS {
        let failed = yuna.call("capture", json!({"seat": SEAT}));
        assert_eq!(failed["isError"], json!(true), "{failed}");
        assert_eq!(
            failed["content"][0]["text"],
            json!("the capture's encoder failed"),
            "{failed}"
        );
    }
    let captured = yuna.call("capture", json!({"seat": SEAT}));
    assert_eq!(captured["isError"], json!(false), "{captured}");
    assert!(
        captured["content"]
            .as_array()
            .is_some_and(|parts| parts.iter().any(|part| part["type"] == json!("image"))),
        "{captured}"
    );
    assert!(
        yuna.log().contains("panicked at"),
        "no encoder panicked\n{}",
        yuna.log()
    );
    let status = yuna.close_and_wait();
    assert!(status.success(), "{status}\n{}", yuna.log());
}

/// A folder another put at the homes' folder's name, once Yuna's was removed, is not Yuna's: a
/// panic of Yuna's state leaves it, as a stop does.
#[cfg(debug_assertions)]
#[test]
fn a_panic_leaves_a_folder_put_at_the_homes_folders_name() {
    let mut yuna = Yuna::launch("panic-replaced", None, PANIC_ON_CLOCK);
    let created = yuna.call("seat", json!({"action": "create"}));
    assert_eq!(
        created["structuredContent"]["seat"],
        json!(SEAT),
        "{created}"
    );
    let home = start_in_home(&mut yuna);
    assert!(home.starts_with(yuna.homes_folder()));
    fs::remove_dir_all(yuna.homes_folder()).expect("removed, as a cleaner does");
    let theirs = yuna.homes_folder();
    fs::DirBuilder::new()
        .mode(0o700)
        .create(&theirs)
        .expect("another's folder");
    fs::write(theirs.join("theirs"), "").expect("another's file");
    yuna.send("clock", json!({}));
    let status = yuna.wait_for_exit();
    assert_eq!(status.code(), Some(101), "{status}\n{}", yuna.log());
    assert!(
        theirs.join("theirs").exists(),
        "the panic removed another's folder at the homes' name"
    );
}

/// The same, stopped on purpose.
#[test]
fn a_stop_leaves_a_folder_put_at_the_homes_folders_name() {
    let mut yuna = Yuna::start("stop-replaced");
    let home = start_in_home(&mut yuna);
    assert!(home.starts_with(yuna.homes_folder()));
    fs::remove_dir_all(yuna.homes_folder()).expect("removed, as a cleaner does");
    let theirs = yuna.homes_folder();
    fs::DirBuilder::new()
        .mode(0o700)
        .create(&theirs)
        .expect("another's folder");
    fs::write(theirs.join("theirs"), "").expect("another's file");
    let status = yuna.close_and_wait();
    assert!(status.success(), "{status}\n{}", yuna.log());
    assert!(
        theirs.join("theirs").exists(),
        "the stop removed another's folder"
    );
}
