//! The tests of the programs' environment, logs, folders and processes.

use std::ffi::OsStr;

use super::folder::{HOMES_FOLDER, PROGRAMS_FOLDER};
use super::homes::{add_home_variables, needs_home};
use super::processes::living_descendants;
use super::*;
use crate::process::{ProcessStat, read_stat};

/// Yuna's package sets its own libraries and Xwayland for Yuna alone, and names in
/// `YUNA_WRAPPED_VARIABLES` what it changed, with the caller's own value of each it found:
/// a program gets the caller's values back, and none where the caller had none.
#[test]
fn a_program_gets_back_what_the_package_changed_for_yuna() {
    let inherited = [
        ("PATH", "/package/xwayland/bin:/usr/bin"),
        ("LD_LIBRARY_PATH", "/package/libglvnd/lib"),
        (
            "__EGL_VENDOR_LIBRARY_FILENAMES",
            "/package/mesa/50_mesa.json",
        ),
        (
            "YUNA_WRAPPED_VARIABLES",
            "PATH LD_LIBRARY_PATH __EGL_VENDOR_LIBRARY_FILENAMES",
        ),
        ("YUNA_CALLER_PATH", "/usr/bin"),
    ]
    .into_iter()
    .map(|(name, value)| (OsString::from(name), OsString::from(value)));
    let environment = program_environment(
        inherited,
        Path::new("/run/user/1000/yuna-mcp-7/runtime/1"),
        Path::new("/run/user/1000/yuna-mcp-7/seat-3"),
        None,
        SeatId(3),
        &BTreeMap::new(),
    );
    assert_eq!(environment[OsStr::new("PATH")], "/usr/bin");
    for gone in [
        "LD_LIBRARY_PATH",
        "__EGL_VENDOR_LIBRARY_FILENAMES",
        "YUNA_WRAPPED_VARIABLES",
        "YUNA_CALLER_PATH",
    ] {
        assert!(!environment.contains_key(OsStr::new(gone)), "{gone}");
    }
}

#[test]
fn a_program_keeps_only_the_variables_it_needs_and_gets_its_own() {
    let inherited = [
        ("PATH", "/bin"),
        ("DISPLAY", ":0"),
        ("DBUS_SESSION_BUS_ADDRESS", "unix:path=/run/user/1000/bus"),
        ("WAYLAND_DISPLAY", "wayland-1"),
        ("XDG_RUNTIME_DIR", "/run/user/1000"),
        ("LC_TIME", "fr_FR.UTF-8"),
        ("VK_ICD_FILENAMES", "/icd.json"),
        ("SSH_AUTH_SOCK", "/run/agent"),
        ("TERM", "xterm"),
    ]
    .into_iter()
    .map(|(name, value)| (OsString::from(name), OsString::from(value)));
    let added = BTreeMap::from([("GDK_BACKEND".to_owned(), "wayland".to_owned())]);
    let environment = program_environment(
        inherited,
        Path::new("/run/user/1000/yuna-mcp-7/runtime/1"),
        Path::new("/run/user/1000/yuna-mcp-7/seat-3"),
        None,
        SeatId(3),
        &added,
    );
    let names: Vec<&str> = environment
        .keys()
        .map(|name| name.to_str().expect("UTF-8"))
        .collect();
    assert_eq!(
        names,
        [
            "GDK_BACKEND",
            "LC_TIME",
            "PATH",
            "VK_ICD_FILENAMES",
            "WAYLAND_DISPLAY",
            "XDG_RUNTIME_DIR",
            "XDG_SESSION_TYPE",
            "YUNA_SEAT"
        ]
    );
    assert_eq!(
        environment[OsStr::new("WAYLAND_DISPLAY")],
        "/run/user/1000/yuna-mcp-7/seat-3"
    );
    assert_eq!(environment[OsStr::new("YUNA_SEAT")], "3");
    assert_eq!(
        environment[OsStr::new("XDG_RUNTIME_DIR")],
        "/run/user/1000/yuna-mcp-7/runtime/1"
    );
}

#[test]
fn the_agents_variables_win_but_the_seats() {
    let inherited = [(OsString::from("PATH"), OsString::from("/bin"))].into_iter();
    let added = BTreeMap::from([
        ("PATH".to_owned(), "/opt/bin".to_owned()),
        ("WAYLAND_DISPLAY".to_owned(), "elsewhere".to_owned()),
        ("YUNA_SEAT".to_owned(), "9".to_owned()),
    ]);
    let environment = program_environment(
        inherited,
        Path::new("/r"),
        Path::new("/w"),
        None,
        SeatId(2),
        &added,
    );
    assert_eq!(environment[OsStr::new("PATH")], "/opt/bin");
    assert_eq!(environment[OsStr::new("WAYLAND_DISPLAY")], "elsewhere");
    // Its seat's, whatever the agent says: it decides which seat's closing ends it.
    assert_eq!(environment[OsStr::new("YUNA_SEAT")], "2");
}

#[test]
fn a_program_gets_its_seats_x_display_and_cookie_not_yunas() {
    let inherited = [
        (OsString::from("DISPLAY"), OsString::from(":0")),
        (
            OsString::from("XAUTHORITY"),
            OsString::from("/home/user/.Xauthority"),
        ),
    ]
    .into_iter();
    let x11 = X11Display {
        number: 12,
        xauthority: PathBuf::from("/run/user/1000/yuna-mcp-7/seat-2.x11/Xauthority"),
        server: None,
    };
    let environment = program_environment(
        inherited,
        Path::new("/r"),
        Path::new("/w"),
        Some(&x11),
        SeatId(2),
        &BTreeMap::new(),
    );
    assert_eq!(environment[OsStr::new("DISPLAY")], ":12");
    assert_eq!(
        environment[OsStr::new("XAUTHORITY")],
        "/run/user/1000/yuna-mcp-7/seat-2.x11/Xauthority"
    );
    // With a DISPLAY and no bus, D-Bus's library would launch a bus of its own through X11,
    // which would hold the seat's Xwayland for ever.
    assert_eq!(
        environment[OsStr::new("DBUS_SESSION_BUS_ADDRESS")],
        "disabled:"
    );
    // The agent's win, as for WAYLAND_DISPLAY.
    let added = BTreeMap::from([("DISPLAY".to_owned(), ":5".to_owned())]);
    let environment = program_environment(
        std::iter::empty(),
        Path::new("/r"),
        Path::new("/w"),
        Some(&x11),
        SeatId(2),
        &added,
    );
    assert_eq!(environment[OsStr::new("DISPLAY")], ":5");
}

/// With accessibility, a program gets its seat's bus and the switches that turn its toolkit
/// on; without, the two that keep its toolkit from looking; an agent's own value of any of
/// them wins, as for every variable.
#[test]
fn a_program_gets_its_seats_bus_or_the_switches_that_keep_its_toolkit_off() {
    let mut accessible = BTreeMap::new();
    add_accessibility_variables(
        &mut accessible,
        Some("unix:path=/r/seat-2.a11y/bus"),
        &BTreeMap::new(),
    );
    assert_eq!(
        accessible[OsStr::new(BUS_VARIABLE)],
        "unix:path=/r/seat-2.a11y/bus"
    );
    for (name, value) in ACCESSIBILITY_VARIABLES {
        assert_eq!(accessible[OsStr::new(name)], *value, "{name}");
    }
    assert!(!accessible.contains_key(OsStr::new("NO_AT_BRIDGE")));
    let mut plain = BTreeMap::new();
    add_accessibility_variables(&mut plain, None, &BTreeMap::new());
    assert_eq!(plain[OsStr::new("NO_AT_BRIDGE")], "1");
    assert_eq!(plain[OsStr::new("GTK_A11Y")], "none");
    assert!(!plain.contains_key(OsStr::new(BUS_VARIABLE)));
    // The agent's own choice stays: a GTK program asked to use AccessKit.
    let added = BTreeMap::from([("GTK_A11Y".to_owned(), "accesskit".to_owned())]);
    let mut chosen = BTreeMap::from([(OsString::from("GTK_A11Y"), OsString::from("accesskit"))]);
    add_accessibility_variables(&mut chosen, None, &added);
    assert_eq!(chosen[OsStr::new("GTK_A11Y")], "accesskit");
}

/// A seat's dbus-daemon gets Yuna's kept variables, no session bus and its seat's
/// `YUNA_SEAT`: never Yuna's own display, socket or bus.
#[test]
fn a_seats_daemon_gets_the_kept_variables_and_no_session_bus() {
    let inherited = [
        ("PATH", "/bin"),
        ("DISPLAY", ":0"),
        ("WAYLAND_DISPLAY", "wayland-1"),
        ("DBUS_SESSION_BUS_ADDRESS", "unix:path=/run/user/1000/bus"),
        ("AT_SPI_BUS_ADDRESS", "unix:path=/run/user/1000/at-spi/bus"),
    ]
    .into_iter()
    .map(|(name, value)| (OsString::from(name), OsString::from(value)));
    let environment = daemon_environment(inherited, SeatId(3));
    let names: Vec<&str> = environment
        .keys()
        .map(|name| name.to_str().expect("UTF-8"))
        .collect();
    assert_eq!(names, ["DBUS_SESSION_BUS_ADDRESS", "PATH", "YUNA_SEAT"]);
    assert_eq!(
        environment[OsStr::new("DBUS_SESSION_BUS_ADDRESS")],
        "disabled:"
    );
    assert_eq!(environment[OsStr::new("YUNA_SEAT")], "3");
}

#[test]
fn a_stat_line_is_read_past_a_name_with_spaces_and_parentheses() {
    let stat = |parent, group, ended| {
        Some(ProcessStat {
            parent,
            group,
            ended,
        })
    };
    assert_eq!(read_stat("42 (a b) c)) S 7 40 42 0"), stat(7, 40, false));
    assert_eq!(read_stat("43 (z) Z 1 43"), stat(1, 43, true));
    assert_eq!(read_stat("44 no parenthesis"), None);
    assert_eq!(read_stat("45 (x) S 1"), None);
}

/// A process its parent is reaping shows `X` in `/proc` for a moment before it goes: it has
/// ended, as a zombie has.
#[test]
fn a_process_being_reaped_has_ended() {
    let ended = read_stat("46 (sleep) X 7 46").expect("a stat line");
    assert!(ended.ended);
    let running = read_stat("47 (sleep) R 7 47").expect("a stat line");
    assert!(!running.ended);
}

/// A log in a file of its own, in the test's temporary folder.
fn log_in(name: &str) -> (ProgramLog, PathBuf) {
    let path = std::env::temp_dir().join(format!("yuna-program-log-{name}-{}", std::process::id()));
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(true)
        .open(&path)
        .expect("a log file");
    (ProgramLog::new(file), path)
}

/// The bytes `start..end` of an output where byte `n` is `n % 251`: a prime, so that bytes
/// read at a wrong place in the ring differ.
fn output(start: u64, end: u64) -> Vec<u8> {
    (start..end).map(|n| (n % 251) as u8).collect()
}

#[test]
fn a_log_keeps_its_head_and_its_newest_bytes_by_offset() {
    let (mut log, path) = log_in("ring");
    let size = LOG_HEAD + 3 * LOG_TAIL + 12_345;
    // Written in uneven pieces, so that pieces cross the head's end and the ring's.
    let mut written = 0;
    for piece in [100, LOG_HEAD - 50, 70_001, LOG_TAIL, 2 * LOG_TAIL] {
        let end = (written + piece).min(size);
        log.append(&output(written, end));
        written = end;
    }
    log.append(&output(written, size));
    let _ = fs::remove_file(&path);
    assert_eq!(log.size, size);
    assert_eq!(log.kept(), LOG_HEAD + LOG_TAIL);
    assert_eq!(log.dropped(), size - LOG_HEAD - LOG_TAIL);
    // The text is the bytes as UTF-8, the invalid ones replaced.
    let text = |start, end| String::from_utf8_lossy(&output(start, end)).into_owned();
    let head = log.read(Some(0), 1_000).expect("a read");
    assert_eq!((head.from, head.to), (0, 1_000));
    assert_eq!(head.text, text(0, 1_000));
    let edge = log.read(Some(LOG_HEAD - 10), 1_000).expect("a read");
    assert_eq!((edge.from, edge.to), (LOG_HEAD - 10, LOG_HEAD));
    let tail_start = size - LOG_TAIL;
    let gap = log.read(Some(LOG_HEAD + 5), 4_000).expect("a read");
    assert_eq!((gap.from, gap.to), (tail_start, tail_start + 4_000));
    assert_eq!(gap.text, text(tail_start, tail_start + 4_000));
    let newest = log.read(None, LOG_TAIL).expect("a read");
    assert_eq!((newest.from, newest.to), (tail_start, size));
    assert_eq!(newest.text, text(tail_start, size));
    let past = log.read(Some(size * 2), 10).expect("a read");
    assert_eq!((past.from, past.to, past.text.as_str()), (size, size, ""));
}

#[test]
fn a_short_log_keeps_everything_and_reads_across_head_and_tail() {
    let (mut log, path) = log_in("short");
    let size = LOG_HEAD + 1_000;
    log.append(&vec![b'a'; size as usize]);
    let whole = log.read(Some(0), LOG_HEAD + LOG_TAIL).expect("a read");
    let _ = fs::remove_file(&path);
    assert_eq!((whole.from, whole.to, whole.dropped), (0, size, 0));
    assert_eq!(whole.text.len(), size as usize);
    assert!(!whole.ended);
    let newest = log.read(None, 2_000).expect("a read");
    assert_eq!((newest.from, newest.to), (size - 2_000, size));
}

#[test]
fn only_yuna_mcps_folders_are_swept() {
    assert!(PROGRAMS_FOLDER.names("yuna-mcp-123"));
    assert!(PROGRAMS_FOLDER.names("yuna-mcp-123-4"));
    // The random name a folder takes when its own is taken.
    assert!(PROGRAMS_FOLDER.names("yuna-mcp-123-18446744073709551615"));
    for other in [
        "yuna-mcp-",
        "yuna-mcp-12-",
        "yuna-mcp-+12",
        "yuna-mcp-test-tree-12",
        "yuna-mcp-12-a",
        ".yuna-mcp-12-starting",
        ".yuna-mcp-12-18446744073709551615-starting",
        "wayland-1",
        "yuna-mcp-homes-12",
    ] {
        assert!(!PROGRAMS_FOLDER.names(other), "{other}");
    }
}

/// The homes' folders are swept by their own names, never a programs' folder; the fatal path
/// takes its own process's alone.
#[test]
fn only_yuna_mcps_homes_are_swept_and_its_own_named() {
    assert!(HOMES_FOLDER.names("yuna-mcp-homes-123"));
    assert!(HOMES_FOLDER.names("yuna-mcp-homes-123-4"));
    assert!(HOMES_FOLDER.names("yuna-mcp-homes-123-18446744073709551615"));
    for other in [
        "yuna-mcp-123",
        "yuna-mcp-homes-",
        "yuna-mcp-homes-12-",
        "yuna-mcp-homes-notes",
        ".yuna-mcp-homes-12-starting",
        ".yuna-mcp-homes-12-18446744073709551615-starting",
    ] {
        assert!(!HOMES_FOLDER.names(other), "{other}");
    }
    assert!(HOMES_FOLDER.names_for("yuna-mcp-homes-123", 123));
    assert!(HOMES_FOLDER.names_for("yuna-mcp-homes-123-2", 123));
    assert!(HOMES_FOLDER.names_for("yuna-mcp-homes-123-18446744073709551615", 123));
    for other in [
        "yuna-mcp-homes-1234",
        "yuna-mcp-homes-12",
        "yuna-mcp-homes-124-1",
    ] {
        assert!(!HOMES_FOLDER.names_for(other, 123), "{other}");
    }
}

/// A program's home is its seat's: `HOME` and the four XDG base folders in it, the defaults of
/// the XDG base directory specification, each one the agent names its own; the user's never
/// reach it, and the system's folders stay Yuna's. A program whose agent names all five needs
/// no home.
#[test]
fn a_program_gets_its_seats_home_and_keeps_the_systems_folders() {
    let inherited = [
        ("HOME", "/home/user"),
        ("XDG_CONFIG_HOME", "/home/user/.config"),
        ("XDG_DATA_HOME", "/home/user/.local/share"),
        ("XDG_CACHE_HOME", "/home/user/.cache"),
        ("XDG_STATE_HOME", "/home/user/.local/state"),
        ("XDG_CONFIG_DIRS", "/etc/xdg"),
        ("XDG_DATA_DIRS", "/usr/share"),
    ]
    .into_iter()
    .map(|(name, value)| (OsString::from(name), OsString::from(value)));
    let added = BTreeMap::from([("XDG_CACHE_HOME".to_owned(), "/own/cache".to_owned())]);
    let mut environment = program_environment(
        inherited,
        Path::new("/r"),
        Path::new("/w"),
        None,
        SeatId(3),
        &added,
    );
    let home = Path::new("/tmp/yuna-mcp-homes-7/seat-3");
    add_home_variables(&mut environment, home, &added);
    let variable = |name: &str| environment[OsStr::new(name)].to_str().expect("UTF-8");
    assert_eq!(variable("HOME"), "/tmp/yuna-mcp-homes-7/seat-3");
    assert_eq!(
        variable("XDG_CONFIG_HOME"),
        "/tmp/yuna-mcp-homes-7/seat-3/.config"
    );
    assert_eq!(
        variable("XDG_DATA_HOME"),
        "/tmp/yuna-mcp-homes-7/seat-3/.local/share"
    );
    assert_eq!(variable("XDG_CACHE_HOME"), "/own/cache");
    assert_eq!(
        variable("XDG_STATE_HOME"),
        "/tmp/yuna-mcp-homes-7/seat-3/.local/state"
    );
    assert_eq!(variable("XDG_CONFIG_DIRS"), "/etc/xdg");
    assert_eq!(variable("XDG_DATA_DIRS"), "/usr/share");

    assert!(needs_home(&added));
    let all: BTreeMap<String, String> = [
        "HOME",
        "XDG_CONFIG_HOME",
        "XDG_DATA_HOME",
        "XDG_CACHE_HOME",
        "XDG_STATE_HOME",
    ]
    .into_iter()
    .map(|name| (name.to_owned(), format!("/own/{name}")))
    .collect();
    assert!(!needs_home(&all));
    let environment = program_environment(
        std::iter::once((OsString::from("HOME"), OsString::from("/home/user"))),
        Path::new("/r"),
        Path::new("/w"),
        None,
        SeatId(3),
        &all,
    );
    assert_eq!(environment[OsStr::new("HOME")], "/own/HOME");
}

#[test]
fn this_process_is_not_its_own_descendant() {
    let me = rustix::process::getpid();
    assert!(!living_descendants(me).contains(&me));
}
