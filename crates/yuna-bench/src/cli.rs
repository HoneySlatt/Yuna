//! The command line: `headless` runs the cases, `report` summarises run files, and the bench's
//! own clients and launcher run as subcommands of the same program.

use std::fs::{self, File};
use std::io::{self, BufWriter, Write as _};
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::lock;
use crate::machine::MachineInfo;
use crate::recipes::{self, Configuration};
use crate::record::{Case, FORMAT, RunFile};
use crate::report;
use crate::sandbox::{self, RENDER_NODE, ShellPaths};
use crate::scenarios::{self, Bench, Durations, REST_WINDOWS, SCENARIOS};

const HELP: &str = "\
yuna-bench: the rivals' bench (ADR-0075). Runs compositors headless, each in a sandbox of its own,
through the same scenarios with the same clients, and reads their costs from outside.

  yuna-bench headless [options]     run the cases, write a run file and its summary
  yuna-bench report <file.json>...  print the summary of run files, in Markdown
  yuna-bench parity run [options]   compare two builds of Yuna (ADR-0142); parity --help

Options of headless (each list separated by commas):
  --compositor <list>     sway, sway-vulkan, kwin, yuna, dwl, mango, umbriel, hyprland, mutter
                          (default: sway,kwin,yuna); niri and cosmic-comp run on the tour only
  --scenario <list>       rest, game, film, pacing, motion (default: all)
  --windows <list>        rest's still windows (default: 1,15,150)
  --configuration <list>  as-shipped, equal-work (default: both)
  --runs <n>              runs of each case, in a random order (default: 3)
  --seed <n>              the order's seed (default: from the clock); recorded
  --seconds <s>           every measured window this long, to try the bench: not comparable
  --out <folder>          where the run file goes (default: target/yuna-bench)
  --keep                  keep the run's folder of logs, which an error keeps anyway

Run it in the bench's shell: nix develop .#bench -c cargo run --release -p yuna-bench -- headless
It takes the agents' exclusive lock itself, or uses the one run.sh --exclusive holds for it.
";

/// The options of `headless`.
#[derive(Debug, Clone, PartialEq)]
pub struct Options {
    /// The recipes.
    pub compositors: Vec<String>,
    /// The scenarios.
    pub scenarios: Vec<String>,
    /// `rest`'s windows.
    pub windows: Vec<u32>,
    /// The configurations.
    pub configurations: Vec<Configuration>,
    /// Runs of each case.
    pub runs: u32,
    /// The order's seed.
    pub seed: Option<u64>,
    /// Every window's length, to try the bench.
    pub seconds: Option<f64>,
    /// Where the run file goes.
    pub out: Option<PathBuf>,
    /// Whether to keep the run's folder.
    pub keep: bool,
}

fn list(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_owned)
        .collect()
}

/// Reads `headless`'s options.
pub fn read_options(arguments: &[String]) -> Result<Options, String> {
    let mut options = Options {
        compositors: vec!["sway".to_owned(), "kwin".to_owned(), "yuna".to_owned()],
        scenarios: SCENARIOS.iter().map(|s| (*s).to_owned()).collect(),
        windows: REST_WINDOWS.to_vec(),
        configurations: Configuration::ALL.to_vec(),
        runs: 3,
        seed: None,
        seconds: None,
        out: None,
        keep: false,
    };
    let mut arguments = arguments.iter();
    while let Some(argument) = arguments.next() {
        let mut value = || {
            arguments
                .next()
                .cloned()
                .ok_or_else(|| format!("{argument} needs a value"))
        };
        match argument.as_str() {
            "--compositor" => options.compositors = list(&value()?),
            "--scenario" => options.scenarios = list(&value()?),
            "--windows" => {
                options.windows = list(&value()?)
                    .iter()
                    .map(|n| n.parse().ok().filter(|&n| n > 0))
                    .collect::<Option<_>>()
                    .ok_or("--windows takes whole numbers above 0")?;
            }
            "--configuration" => {
                options.configurations = list(&value()?)
                    .iter()
                    .map(|name| Configuration::named(name))
                    .collect::<Option<_>>()
                    .ok_or("--configuration takes as-shipped and equal-work")?;
            }
            "--runs" => {
                options.runs = value()?
                    .parse()
                    .ok()
                    .filter(|&n| n > 0)
                    .ok_or("--runs takes a whole number above 0")?;
            }
            "--seed" => options.seed = Some(value()?.parse().map_err(|_| "--seed takes a number")?),
            "--seconds" => {
                options.seconds = Some(
                    value()?
                        .parse()
                        .ok()
                        .filter(|s: &f64| *s > 0.0)
                        .ok_or("--seconds takes a number above 0")?,
                );
            }
            "--out" => options.out = Some(PathBuf::from(value()?)),
            "--keep" => options.keep = true,
            other => return Err(format!("unknown argument {other:?}")),
        }
    }
    for name in &options.compositors {
        if let Some(rival) = recipes::tour_only(name) {
            return Err(format!("{name} runs on the tour only: {}", rival.why));
        }
        if recipes::recipe(name).is_none() {
            return Err(format!("no recipe {name:?}"));
        }
    }
    for name in &options.scenarios {
        if !SCENARIOS.contains(&name.as_str()) {
            return Err(format!("no scenario {name:?}"));
        }
    }
    if options.compositors.is_empty() || options.scenarios.is_empty() || options.windows.is_empty()
    {
        return Err("--compositor, --scenario and --windows need one value at least".to_owned());
    }
    Ok(options)
}

/// Every case the options ask for, each `runs` times, in a random order from `seed`.
pub fn cases(options: &Options, seed: u64) -> Vec<(Case, u32)> {
    let mut cases = Vec::new();
    for compositor in &options.compositors {
        for configuration in &options.configurations {
            for scenario in &options.scenarios {
                let windows: Vec<Option<u32>> = if scenario == "rest" {
                    options.windows.iter().map(|w| Some(*w)).collect()
                } else {
                    vec![None]
                };
                for windows in windows {
                    for repeat in 1..=options.runs {
                        cases.push((
                            Case {
                                compositor: compositor.clone(),
                                configuration: configuration.name().to_owned(),
                                scenario: scenario.clone(),
                                windows,
                            },
                            repeat,
                        ));
                    }
                }
            }
        }
    }
    shuffle(&mut cases, seed);
    cases
}

/// SplitMix64: a small generator whose sequence a seed fixes, for the cases' order, and the
/// parity script's.
pub(crate) struct SplitMix(pub(crate) u64);

impl SplitMix {
    pub(crate) fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }
}

/// Shuffles `items` by Fisher and Yates, from `seed`.
pub(crate) fn shuffle<T>(items: &mut [T], seed: u64) {
    shuffle_with(items, &mut SplitMix(seed));
}

/// Shuffles `items` by Fisher and Yates, drawing from `random`.
pub(crate) fn shuffle_with<T>(items: &mut [T], random: &mut SplitMix) {
    for i in (1..items.len()).rev() {
        let j = (random.next() % (i as u64 + 1)) as usize;
        items.swap(i, j);
    }
}

/// A time as ISO 8601 in UTC, to the second.
pub fn utc(time: SystemTime) -> String {
    let seconds = time.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let (days, rest) = ((seconds / 86_400) as i64, seconds % 86_400);
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rest / 3600,
        rest % 3600 / 60,
        rest % 60
    )
}

/// Writes `path` with `write`, through a temporary file beside it, flushed to the disk, and a
/// rename: the file is never left half written, and an earlier version stays whole until the new
/// one replaces it.
pub(crate) fn write_atomically(
    path: &Path,
    write: impl FnOnce(&mut BufWriter<File>) -> io::Result<()>,
) -> Result<(), String> {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".partial");
    let temporary = path.with_file_name(name);
    let written = (|| {
        let mut writer = BufWriter::new(File::create(&temporary)?);
        write(&mut writer)?;
        let file = writer
            .into_inner()
            .map_err(io::IntoInnerError::into_error)?;
        file.sync_all()?;
        fs::rename(&temporary, path)
    })();
    written.map_err(|e| {
        let _ = fs::remove_file(&temporary);
        format!("{}: {e}", path.display())
    })
}

/// Writes the run file `file` to `path`, atomically, the time it is written in it.
fn write_run_file(path: &Path, file: &mut RunFile) -> Result<(), String> {
    file.written_utc = Some(utc(SystemTime::now()));
    write_atomically(path, |writer| {
        serde_json::to_writer_pretty(writer, file).map_err(io::Error::from)
    })
}

fn headless(arguments: &[String]) -> Result<bool, String> {
    // First, before any thread or child: no descriptor this process inherited, the lock
    // `run.sh --exclusive` holds among them, may reach a compositor or a client.
    let inherited = sandbox::close_inherited_on_exec();
    let options = read_options(arguments)?;
    let shell = ShellPaths::from_environment()?;
    let program = std::env::current_exe().map_err(|e| format!("this program: {e}"))?;
    let mut notes = Vec::new();
    if !inherited.is_empty() {
        notes.push(format!(
            "descriptors inherited and kept from every child: {inherited:?}"
        ));
    }
    // A bench killed earlier left its sandboxes running: they end before the lock is waited for,
    // and before anything is measured.
    let swept = sandbox::sweep_left_behind(&crate::procfs::Proc::system());
    if !swept.is_empty() {
        notes.push(format!(
            "scopes left by a bench killed earlier, killed: {}",
            swept.join(", ")
        ));
    }
    let links = sandbox::sweep_runtime_links(&crate::procfs::Proc::system(), Path::new("/tmp"));
    if !links.is_empty() {
        notes.push(format!(
            "runtime links left by a bench killed earlier, removed: {}",
            links.join(", ")
        ));
    }
    let exclusive = lock::take()?;
    let started = SystemTime::now();
    let seed = options.seed.unwrap_or_else(|| {
        started
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos() as u64)
    });
    // In /tmp itself, not the shell's TMPDIR, which `nix develop` removes when it ends: the folder
    // outlives the bench when it is kept, and its sockets' paths stay under Unix's 108 bytes. Made
    // fresh, the user's alone: an existing one is refused rather than reused.
    let started_s = started
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let folder = Path::new("/tmp").join(format!("yuna-bench-{}-{started_s}", std::process::id()));
    fs::DirBuilder::new()
        .mode(0o700)
        .create(&folder)
        .map_err(|e| format!("{}: {e}", folder.display()))?;
    let durations = options
        .seconds
        .map_or(Durations::STANDARD, Durations::with_windows);
    let mut bench = Bench::new(shell.clone(), program, folder.clone(), durations);
    let compositors: Vec<&str> = options.compositors.iter().map(String::as_str).collect();
    let scenarios_asked: Vec<&str> = options.scenarios.iter().map(String::as_str).collect();
    scenarios::prepare(&mut bench, &compositors, &scenarios_asked, &mut notes);
    let cases = cases(&options, seed);
    let commit = scenarios::commit(&[]);
    let mut file = RunFile {
        format: FORMAT,
        commit: commit
            .as_ref()
            .map(|c| c.trim_end_matches("-dirty").to_owned()),
        dirty: commit.as_ref().map(|c| c.ends_with("-dirty")),
        started_utc: utc(started),
        written_utc: None,
        arguments: arguments.to_vec(),
        seed,
        seconds_override: options.seconds,
        exclusive_lock: exclusive.how.clone(),
        machine: MachineInfo::read(RENDER_NODE, Some(&shell.mesa.to_string_lossy())),
        notes,
        order: cases
            .iter()
            .map(|(case, repeat)| format!("{} #{repeat}", case.label()))
            .collect(),
        runs: Vec::new(),
    };
    // Written before the first case and again after each, so that a run cut short keeps every
    // case it ran: `report` summarises it then.
    let out = options
        .out
        .unwrap_or_else(|| scenarios::workspace().join("target/yuna-bench"));
    fs::create_dir_all(&out).map_err(|e| format!("{}: {e}", out.display()))?;
    let stem = format!("run-{}", file.started_utc.replace([':', '-'], ""));
    let json = out.join(format!("{stem}.json"));
    write_run_file(&json, &mut file)?;
    for (n, (case, repeat)) in cases.iter().enumerate() {
        eprintln!(
            "yuna-bench: [{}/{}] {} run {repeat}",
            n + 1,
            cases.len(),
            case.label()
        );
        let run = scenarios::run_case(&mut bench, case, *repeat);
        if let Some(error) = &run.error {
            eprintln!("yuna-bench:   failed: {error}");
        }
        for refused in &run.refused {
            eprintln!("yuna-bench:   refused: {refused}");
        }
        file.runs.push(run);
        write_run_file(&json, &mut file)?;
    }
    let summary = out.join(format!("{stem}.md"));
    let text = report::summary(&[(format!("{stem}.json"), file.clone())]);
    write_atomically(&summary, |writer| writer.write_all(text.as_bytes()))?;
    let clean = file
        .runs
        .iter()
        .all(|run| run.error.is_none() && run.refused.is_empty());
    if options.keep || !clean {
        eprintln!(
            "yuna-bench: the run's logs are kept in {}",
            folder.display()
        );
    } else {
        let _ = fs::remove_dir_all(&folder);
    }
    println!("{}", json.display());
    println!("{}", summary.display());
    Ok(clean)
}

fn report(paths: &[String]) -> Result<(), String> {
    if paths.is_empty() {
        return Err("report needs one run file at least".to_owned());
    }
    let mut files = Vec::new();
    for path in paths {
        let text = fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
        let file = report::read(&text).map_err(|e| format!("{path}: {e}"))?;
        let name = Path::new(path)
            .file_name()
            .map_or(path.clone(), |n| n.to_string_lossy().into_owned());
        files.push((name, file));
    }
    print!("{}", report::summary(&files));
    Ok(())
}

/// Runs the command line `arguments`, the program's name left out.
pub fn main(arguments: &[String]) -> ExitCode {
    let result = match arguments.first().map(String::as_str) {
        Some("headless") => headless(&arguments[1..]).map(|clean| {
            if clean {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            }
        }),
        Some("report") => report(&arguments[1..]).map(|()| ExitCode::SUCCESS),
        Some("parity") => return crate::parity::main(&arguments[1..]),
        Some("client") => match arguments.get(1..).unwrap_or_default() {
            [still, flag, count] if still == "still" && flag == "--windows" => count
                .parse()
                .map_err(|_| "--windows takes a number".to_owned())
                .and_then(crate::still::run)
                .map(|()| ExitCode::SUCCESS),
            [frames] if frames == "frames" => crate::capture::run().map(|()| ExitCode::SUCCESS),
            [keys] if keys == "keys" => crate::keys::run().map(|()| ExitCode::SUCCESS),
            [lockstep, windows, count, control, socket]
                if lockstep == "lockstep" && windows == "--windows" && control == "--control" =>
            {
                count
                    .parse()
                    .map_err(|_| "--windows takes a number".to_owned())
                    .and_then(|count| crate::parity::lockstep::run(count, Path::new(socket)))
                    .map(|()| ExitCode::SUCCESS)
            }
            _ => Err("client takes `still --windows <n>`, `frames`, `keys` or \
                      `lockstep --windows <n> --control <socket>`"
                .to_owned()),
        },
        Some("parent") => match arguments.get(1..).unwrap_or_default() {
            [file] => sandbox::run_parent(Path::new(file)).map(|ended_well| {
                if ended_well {
                    ExitCode::SUCCESS
                } else {
                    ExitCode::FAILURE
                }
            }),
            _ => Err("parent takes <file>".to_owned()),
        },
        Some("enter-core") => match arguments.get(1).map(String::as_str) {
            Some("--") => sandbox::enter_core(&arguments[2..]).map(|()| ExitCode::SUCCESS),
            _ => Err("enter-core takes -- <program> <arguments>".to_owned()),
        },
        Some("-h" | "--help") | None => {
            print!("{HELP}");
            Ok(ExitCode::SUCCESS)
        }
        Some(other) => Err(format!("unknown command {other:?}; see --help")),
    };
    result.unwrap_or_else(|error| {
        eprintln!("yuna-bench: {error}");
        ExitCode::from(2)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(arguments: &[&str]) -> Result<Options, String> {
        read_options(
            &arguments
                .iter()
                .map(|a| (*a).to_owned())
                .collect::<Vec<_>>(),
        )
    }

    #[test]
    fn options_default_to_every_scenario_three_times() {
        let options = read(&[]).unwrap();
        assert_eq!(options.compositors, ["sway", "kwin", "yuna"]);
        assert_eq!(options.scenarios.len(), 5);
        assert_eq!(options.runs, 3);
        let options = read(&[
            "--compositor",
            "sway,yuna",
            "--scenario",
            "rest",
            "--windows",
            "15",
        ])
        .unwrap();
        assert_eq!(options.compositors, ["sway", "yuna"]);
        assert_eq!(options.windows, [15]);
        assert!(read(&["--compositor", "gamescope"]).is_err());
        let tour_only = read(&["--compositor", "niri"]).unwrap_err();
        assert!(tour_only.contains("tour only"), "{tour_only}");
        assert!(read(&["--compositor", "dwl,hyprland,mutter"]).is_ok());
        assert!(read(&["--scenario", "sleep"]).is_err());
        assert!(read(&["--runs", "0"]).is_err());
        assert!(read(&["--windows", "0"]).is_err());
        assert!(read(&["--configuration", "shipped"]).is_err());
        assert!(read(&["--seconds"]).is_err());
    }

    #[test]
    fn the_cases_order_is_random_and_its_seed_replays_it() {
        let options = read(&["--compositor", "sway,kwin,yuna", "--scenario", "rest,game"]).unwrap();
        let first = cases(&options, 42);
        // 3 compositors × 2 configurations × (3 rests + 1 game) × 3 runs.
        assert_eq!(first.len(), 72);
        assert_eq!(cases(&options, 42), first);
        assert_ne!(cases(&options, 43), first);
        let mut sorted = first.clone();
        sorted.sort();
        let mut unshuffled = cases(&options, 42);
        unshuffled.sort();
        assert_eq!(sorted, unshuffled);
        assert_ne!(first, sorted, "the order is not shuffled");
    }

    #[test]
    fn a_run_file_is_replaced_whole_or_not_at_all() {
        let folder =
            std::env::temp_dir().join(format!("yuna-bench-test-atomic-{}", std::process::id()));
        fs::create_dir_all(&folder).unwrap();
        let path = folder.join("run-x.json");
        let mut file = RunFile {
            format: FORMAT,
            started_utc: "2026-11-16T06:14:01Z".to_owned(),
            ..RunFile::default()
        };
        write_run_file(&path, &mut file).unwrap();
        let first = fs::read_to_string(&path).unwrap();
        let read = report::read(&first).unwrap();
        assert!(read.written_utc.is_some());
        assert!(read.runs.is_empty());
        // A case later, the file holds it, whole.
        file.runs.push(crate::record::CaseRun::default());
        write_run_file(&path, &mut file).unwrap();
        assert_eq!(
            report::read(&fs::read_to_string(&path).unwrap())
                .unwrap()
                .runs
                .len(),
            1
        );
        // A write cut short leaves the last whole file, and no temporary beside it.
        let before = fs::read_to_string(&path).unwrap();
        let cut = write_atomically(&path, |writer| {
            writer.write_all(b"{\"format\": 1, \"ru")?;
            Err(io::Error::other("the disk is full"))
        });
        assert!(cut.unwrap_err().contains("the disk is full"));
        assert_eq!(fs::read_to_string(&path).unwrap(), before);
        let left: Vec<_> = fs::read_dir(&folder)
            .unwrap()
            .flatten()
            .map(|entry| entry.file_name())
            .collect();
        assert_eq!(left, ["run-x.json"]);
        fs::remove_dir_all(&folder).unwrap();
    }

    #[test]
    fn times_are_written_in_utc() {
        assert_eq!(utc(UNIX_EPOCH), "1970-01-01T00:00:00Z");
        let time = UNIX_EPOCH + std::time::Duration::from_secs(1_794_700_800 + 3_723);
        assert_eq!(utc(time), "2026-11-15T01:02:03Z");
        let leap = UNIX_EPOCH + std::time::Duration::from_secs(951_782_400);
        assert_eq!(utc(leap), "2000-02-29T00:00:00Z");
    }
}
