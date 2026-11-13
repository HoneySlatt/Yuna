//! `yuna mcp` as an agent's MCP client runs it: requests on its standard input, replies on its
//! standard output, nothing else there, and the programs it starts
//! ([ADR-0033](../../../docs/adr/0033-the-mcp-server.md),
//! [ADR-0035](../../../docs/adr/0035-agents-seats.md)). The protocol's exchanges are tested in
//! the test's own process (`tests/headless/mcp.rs`); here, what needs a process of its own: the
//! programs' environment, their logs, and a whole tree of processes ended when Yuna stops.

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
        let folder =
            std::env::temp_dir().join(format!("yuna-mcp-test-{name}-{}", std::process::id()));
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(folder.join("runtime"))
            .expect("a runtime folder");
        let mut child = Command::new(env!("CARGO_BIN_EXE_yuna"))
            .args(["mcp", "--output", "1920x1080@60,1"])
            .env("XDG_RUNTIME_DIR", folder.join("runtime"))
            .env("YUNA_LOG", "info")
            .env("DISPLAY", ":99")
            .env("DBUS_SESSION_BUS_ADDRESS", "unix:path=/nonexistent")
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

    /// Calls a tool in 2026-07-28 and returns its result.
    fn call(&mut self, tool: &str, arguments: Value) -> Value {
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

/// A program started through `spawn` gets its seat's socket and a runtime folder of its own, and
/// neither `DISPLAY` nor the session bus; its output goes to its log. A process its program
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
    assert_eq!(variable("DISPLAY"), None);
    assert_eq!(variable("DBUS_SESSION_BUS_ADDRESS"), None);
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
/// log keeps 1 MiB of its output and counts the rest; a program whose first process left while
/// one it started runs is still stopped, its whole group with it.
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
    let flood = format!("head -c {} /dev/zero", (1 << 20) + over);
    yuna.call(
        "spawn",
        json!({"action": "start", "seat": SEAT, "command": ["sh", "-c", flood]}),
    );
    let flooded = wait_for("the flood's last bytes", || {
        let flooded = program(&mut yuna, 1);
        (flooded["state"] == json!("exited") && flooded["log_dropped"] == json!(over))
            .then_some(flooded)
    });
    assert_eq!(flooded["log_bytes"], json!(1 << 20));

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

/// Closing a seat ([ADR-0037](../../../docs/adr/0037-seats-tools-input-and-closing.md)) ends its
/// processes and those alone, and answers once they are gone: its program, which ignores
/// SIGTERM and gets SIGKILL 2 s later, and a daemon that left its parents and its group, found
/// by `YUNA_SEAT`. Another seat's program runs on. A daemon that also empties its environment
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
    let neighbour = neighbour["structuredContent"]["program"]["pid"]
        .as_i64()
        .expect("a pid") as i32;

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
    assert!(alive(neighbour) && alive(escaped));
    assert!(!socket_2.exists(), "its socket is removed");
    let gone = yuna.call("spawn", json!({"action": "list", "seat": SEAT}));
    assert_eq!(gone["isError"], json!(true));
    let seats = yuna.call("seat", json!({"action": "list"}));
    assert_eq!(seats["structuredContent"]["seats"][0]["seat"], json!(3));

    let status = yuna.close_and_wait();
    assert!(status.success(), "{status}\n{}", yuna.log());
    wait_for("every process's end", || {
        (!alive(escaped) && !alive(neighbour)).then_some(())
    });
}
