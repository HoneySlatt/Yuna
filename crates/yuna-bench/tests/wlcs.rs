//! `yuna-bench wlcs` as a program: a WLCS process it starts does not outlive it. The rest of its
//! tests are its modules' own, in `src/wlcs/`.

use std::fs;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// The stand-in for WLCS's runner.
const FAKE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/src/wlcs/fixtures/fake-wlcs.sh"
);

/// Whether process `pid` runs: it exists, and is not a zombie waiting to be reaped.
fn runs(pid: i32) -> bool {
    fs::read_to_string(format!("/proc/{pid}/stat")).is_ok_and(|stat| {
        stat.rsplit_once(") ")
            .is_some_and(|(_, rest)| !rest.starts_with('Z'))
    })
}

#[test]
fn a_wlcs_process_ends_when_the_runner_is_killed() {
    let folder =
        std::env::temp_dir().join(format!("yuna-bench-wlcs-killed-{}", std::process::id()));
    let _ = fs::remove_dir_all(&folder);
    fs::create_dir_all(&folder).unwrap();
    let path = |name: &str| folder.join(name).display().to_string();
    // A test whose process hangs, silent: the runner's own process, `sleep` once it began.
    fs::write(folder.join("scenario"), "Suite.a hang-alone\n").unwrap();
    fs::write(
        folder.join("expected.txt"),
        "format 1\nwlcs 1.8.1\npass Suite.a\n",
    )
    .unwrap();
    let mut runner = Command::new(env!("CARGO_BIN_EXE_yuna-bench"))
        .args(["wlcs", "check", "--listed", "--runner", FAKE])
        .args([
            "--module",
            &path("scenario"),
            "--expected",
            &path("expected.txt"),
        ])
        .args(["--work", &path("work")])
        .env("YUNA_WLCS_VERSION", "1.8.1")
        .stdout(Stdio::null())
        .spawn()
        .unwrap();
    let until = Instant::now() + Duration::from_secs(30);
    let pid = loop {
        if let Ok(text) = fs::read_to_string(folder.join("pid"))
            && text.ends_with('\n')
        {
            break text.trim().parse::<i32>().unwrap();
        }
        assert!(Instant::now() < until, "the test never began");
        std::thread::sleep(Duration::from_millis(20));
    };
    assert!(runs(pid));
    // SIGKILL: nothing of the runner's own code runs after it.
    runner.kill().unwrap();
    runner.wait().unwrap();
    let until = Instant::now() + Duration::from_secs(10);
    while runs(pid) {
        if Instant::now() > until {
            if let Some(pid) = rustix::process::Pid::from_raw(pid) {
                let _ = rustix::process::kill_process(pid, rustix::process::Signal::KILL);
            }
            panic!("the WLCS process {pid} outlived its runner");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    fs::remove_dir_all(&folder).unwrap();
}
