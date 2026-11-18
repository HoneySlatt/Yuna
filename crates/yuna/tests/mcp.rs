//! `yuna mcp` as an agent's MCP client runs it: requests on its standard input, replies on its
//! standard output, nothing else there, and the programs it starts
//! ([ADR-0033](../../../docs/adr/0033-the-mcp-server.md),
//! [ADR-0078](../../../docs/adr/0078-agents-seats-in-one-yuna-frozen-until-stage-2s-core.md)).
//! The protocol's exchanges are tested in the test's own process (`tests/headless/mcp.rs`);
//! here, what needs a process of its own: the programs' environment, their logs, and a whole
//! tree of processes ended when Yuna stops.

#[path = "common/application_environment.rs"]
mod application_environment;

use std::collections::BTreeMap;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::{DirBuilderExt, FileTypeExt};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use application_environment::application_environment;
use serde_json::{Value, json};
use smithay::reexports::rustix::process::{Pid, Signal, kill_process};

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
        let mut yuna = Yuna::start_without_seat(name);
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
        let folder =
            std::env::temp_dir().join(format!("yuna-mcp-test-{name}-{}", std::process::id()));
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(folder.join("runtime"))
            .expect("a runtime folder");
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
            .env("XDG_RUNTIME_DIR", folder.join("runtime"))
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

/// The parent of process `pid`, from `/proc`; none once it is gone.
fn parent_of(pid: i32) -> Option<i32> {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let after_name = &stat[stat.rfind(')')? + 1..];
    let mut fields = after_name.split_whitespace();
    let state = fields.next()?;
    if state == "Z" {
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
#[test]
fn a_file_as_input_is_refused() {
    let folder = std::env::temp_dir().join(format!("yuna-mcp-file-{}", std::process::id()));
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&folder)
        .expect("a runtime folder");
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

/// A `yuna mcp` killed by SIGKILL leaves its folder in `XDG_RUNTIME_DIR`: the next one removes
/// it when it starts, when its lock file's lock is free, whatever pid its name holds, a retry's
/// `yuna-mcp-<pid>-<n>` too. It keeps a folder whose lock is held, though its pid is gone; one
/// with no lock file, an older Yuna's; a link to a folder, which it does not follow; and any
/// other name. A live Yuna holds its own folder's lock.
#[test]
fn an_old_yunas_folder_is_removed_at_start_and_a_living_ones_kept() {
    use yuna::mcp::LOCK_FILE;
    let top = std::env::temp_dir().join(format!("yuna-mcp-test-stale-{}", std::process::id()));
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

    let mut yuna = Yuna::start("stale");
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
    assert!(
        !runtime
            .join(format!(".yuna-mcp-{}-starting", yuna.child.id()))
            .exists()
    );
    drop(held);
    let status = yuna.close_and_wait();
    assert!(status.success(), "{status}\n{}", yuna.log());
}
