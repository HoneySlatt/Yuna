//! Measures what a change of an agent's keyboard layout costs Yuna's state thread, which nothing
//! else runs on meanwhile: `Compositor::set_keyboard_layout`, from the call to its return. In it,
//! XKB compiles the keymap from its files, Yuna reads which keys type each keysym and how the
//! groups switch, Smithay's keyboard compiles the keymap again from its text and writes that text
//! into a file for the clients. Beside it, the two compiles alone: from the names, as
//! `xkb_keymap_new_from_names` takes them, and from the keymap's text, as Smithay's
//! `set_keymap_from_string` takes it.
//!
//! It makes a compositor in its own process, on the manual clock, with one agent's seat and no
//! client, then changes the seat's layout `--changes` times to each of `fr`, `de`, `us,ru` with
//! `grp:alt_shift_toggle`, `us`, and the limit, four layouts with 14 group options in 247 of the
//! 256 bytes an options field holds, in turn, 20 by default, after one change of each to warm the
//! file cache, and prints each layout's median and longest, and all of them together. A client
//! adds a file descriptor sent on its socket for each change, which this leaves out.
//!
//! Then it times `Compositor::type_text`, `--changes` calls of a line of 63 characters on `us`,
//! and the same line with an accented letter and a Cyrillic one on the limit layout, whose groups
//! the line switches: with no modifier, then with Num Lock on, a state whose keys `type` reads
//! again at each call (`keymap.rs`). With no client, the keys reach no surface; what it times is
//! Yuna's planning and Smithay's keyboard.
//!
//! Beside it, it reads what the machine's *other* programs used, every second, with the readers
//! of `yuna_bench::machine`
//! ([ADR-0060](../../../docs/adr/0060-development-first-agents-in-their-own-yuna.md)), and marks
//! the run disturbed by that module's rule. Build and run it in the development shell, in debug
//! and in release, alone among measurements; it opens no display:
//!
//! ```sh
//! nix develop -c cargo run -p yuna --example layout_time
//! nix develop -c cargo run --release -p yuna --example layout_time
//! ```

use std::process::ExitCode;
use std::time::{Duration, Instant};

use xkbcommon::xkb;
use yuna::{ClockMode, Compositor, Config, KeyboardLayout};
use yuna_bench::machine::MachineWatch;
use yuna_bench::sandbox::RENDER_NODE;

/// The largest layout an agent may set: four layouts, and group options up to the bound.
const LIMIT: &str = "us,fr,de,ru";
const LIMIT_OPTIONS: &str = "grp:alt_shift_toggle,grp:ctrl_shift_toggle,grp:win_space_toggle,\
    grp:caps_toggle,grp:shifts_toggle,grp:ctrl_alt_toggle,grp:alt_space_toggle,grp:menu_toggle,\
    grp:sclk_toggle,grp:lwin_toggle,grp:rctrl_toggle,grp:lctrl_toggle,grp:alts_toggle,grp:toggle";

/// The layouts changed to, in turn: their names and options.
const LAYOUTS: [(&str, &str); 5] = [
    ("fr", ""),
    ("de", ""),
    ("us,ru", "grp:alt_shift_toggle"),
    ("us", ""),
    (LIMIT, LIMIT_OPTIONS),
];

/// The line `type` types on `us`: 63 characters, capitals and symbols among them.
const LINE: &str = "The quick brown fox jumps over the lazy dog: 0123456789 (TYPE)!";

/// The changes to each layout by default.
const CHANGES: usize = 20;

fn main() -> ExitCode {
    let changes = match read_changes(std::env::args().skip(1)) {
        Ok(changes) => changes,
        Err(reason) => {
            eprintln!("layout_time: {reason}");
            return ExitCode::from(2);
        }
    };
    match measure(changes) {
        Ok(()) => ExitCode::SUCCESS,
        Err(reason) => {
            eprintln!("layout_time: {reason}");
            ExitCode::FAILURE
        }
    }
}

/// `--changes <n>`, the one option.
fn read_changes(mut args: impl Iterator<Item = String>) -> Result<usize, String> {
    match (args.next().as_deref(), args.next()) {
        (None, _) => Ok(CHANGES),
        (Some("--changes"), Some(n)) => match n.parse() {
            Ok(n) if n > 0 => Ok(n),
            _ => Err(format!("--changes takes a positive number, not {n:?}")),
        },
        (Some(other), _) => Err(format!("no option {other:?}: --changes <n>")),
    }
}

fn layout(name: &str, options: &str) -> KeyboardLayout {
    KeyboardLayout {
        layout: name.to_owned(),
        variant: String::new(),
        options: options.to_owned(),
    }
}

fn measure(changes: usize) -> Result<(), String> {
    let mut compositor = Compositor::new(Config {
        outputs: Vec::new(),
        clock: ClockMode::Manual,
        listen: false,
        ..Config::default()
    })
    .map_err(|e| e.to_string())?;
    let seat = compositor.add_seat().map_err(|e| e.to_string())?;
    let profile = if cfg!(debug_assertions) {
        "debug"
    } else {
        "release"
    };
    println!("{profile} build, {changes} changes to each layout, after one each to warm up");

    let watch = MachineWatch::start(RENDER_NODE);
    for (name, options) in LAYOUTS {
        let started = Instant::now();
        compositor
            .set_keyboard_layout(seat, layout(name, options))
            .map_err(|e| e.to_string())?;
        println!("first change to {name}: {}", millis(started.elapsed()));
    }
    let mut changed: Vec<Vec<Duration>> = vec![Vec::new(); LAYOUTS.len()];
    let mut compiled: Vec<Vec<Duration>> = vec![Vec::new(); LAYOUTS.len()];
    let mut from_text: Vec<Vec<Duration>> = vec![Vec::new(); LAYOUTS.len()];
    let context = xkb::Context::new(xkb::CONTEXT_NO_ENVIRONMENT_NAMES);
    for _ in 0..changes {
        for (index, (name, options)) in LAYOUTS.into_iter().enumerate() {
            let started = Instant::now();
            compositor
                .set_keyboard_layout(seat, layout(name, options))
                .map_err(|e| e.to_string())?;
            changed[index].push(started.elapsed());

            let started = Instant::now();
            let keymap = xkb::Keymap::new_from_names(
                &context,
                "evdev",
                "pc105",
                name,
                "",
                Some(options.to_owned()),
                xkb::KEYMAP_COMPILE_NO_FLAGS,
            );
            compiled[index].push(started.elapsed());
            let keymap = keymap.ok_or_else(|| format!("{name} does not compile"))?;

            let started = Instant::now();
            let text = keymap.get_as_string(xkb::KEYMAP_FORMAT_TEXT_V1);
            let again = xkb::Keymap::new_from_string(
                &context,
                text,
                xkb::KEYMAP_FORMAT_TEXT_V1,
                xkb::KEYMAP_COMPILE_NO_FLAGS,
            );
            from_text[index].push(started.elapsed());
            again.ok_or_else(|| format!("{name}'s text does not compile"))?;
        }
    }
    let mut typing = Vec::new();
    let limit_line = format!("{LINE} é ж");
    for (name, options, line) in [
        ("us", "", LINE),
        (LIMIT, LIMIT_OPTIONS, limit_line.as_str()),
    ] {
        compositor
            .set_keyboard_layout(seat, layout(name, options))
            .map_err(|e| e.to_string())?;
        let plain = time_typing(&mut compositor, seat, line, changes)?;
        compositor
            .tap_keys(seat, "Num_Lock")
            .map_err(|e| e.to_string())?;
        let num_lock = time_typing(&mut compositor, seat, line, changes)?;
        compositor
            .tap_keys(seat, "Num_Lock")
            .map_err(|e| e.to_string())?;
        typing.push((name, line.chars().count(), plain, num_lock));
    }
    let load = watch.finish();

    for (index, (name, options)) in LAYOUTS.into_iter().enumerate() {
        let label = match options {
            "" => name.to_owned(),
            LIMIT_OPTIONS => format!("{name} with 14 group options, {} bytes", options.len()),
            _ => format!("{name} {options}"),
        };
        println!(
            "{label}: change {}; XKB's compile alone {}; from its text {}",
            summary(&changed[index]),
            summary(&compiled[index]),
            summary(&from_text[index])
        );
    }
    let all_changes: Vec<Duration> = changed.concat();
    let all_compiles: Vec<Duration> = compiled.concat();
    let all_from_text: Vec<Duration> = from_text.concat();
    println!(
        "all {} changes: {}; XKB's compile alone {}; from its text {}",
        all_changes.len(),
        summary(&all_changes),
        summary(&all_compiles),
        summary(&all_from_text)
    );
    for (name, characters, plain, num_lock) in typing {
        println!(
            "type {characters} characters on {name}: plain {}; Num Lock on {}",
            summary(&plain),
            summary(&num_lock)
        );
    }
    let cores = |value: Option<f64>| value.map_or("unknown".to_owned(), |v| format!("{v:.2}"));
    println!(
        "other programs: {} cores on average, {} at most; GPU {} % on average; disturbed: {}",
        cores(load.others_cpu_cores_mean),
        cores(load.others_cpu_cores_max),
        cores(load.others_gpu_percent_mean),
        load.disturbed
            .map_or("unknown".to_owned(), |disturbed| disturbed.to_string())
    );
    Ok(())
}

/// `calls` calls typing `line` on `seat`, each timed.
fn time_typing(
    compositor: &mut Compositor,
    seat: yuna_types::SeatId,
    line: &str,
    calls: usize,
) -> Result<Vec<Duration>, String> {
    (0..calls)
        .map(|_| {
            let started = Instant::now();
            compositor
                .type_text(seat, line)
                .map_err(|e| e.to_string())?;
            Ok(started.elapsed())
        })
        .collect()
}

/// The median and the longest of `times`.
fn summary(times: &[Duration]) -> String {
    let mut sorted = times.to_vec();
    sorted.sort_unstable();
    let median = match sorted.len() {
        0 => Duration::ZERO,
        n if n % 2 == 1 => sorted[n / 2],
        n => (sorted[n / 2 - 1] + sorted[n / 2]) / 2,
    };
    let longest = sorted.last().copied().unwrap_or_default();
    format!("median {}, longest {}", millis(median), millis(longest))
}

fn millis(time: Duration) -> String {
    format!("{:.2} ms", time.as_secs_f64() * 1e3)
}
