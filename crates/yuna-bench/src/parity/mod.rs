//! The parity script, `yuna-bench parity run`: two builds of Yuna compared on figures the
//! machine's load does not move, by the rule of
//! [ADR-0142](../../../../docs/adr/0142-parity-a-pinned-baseline-figures-in-classes-three-verdicts.md).
//! `docs/ARCHITECTURE.md` (*The parity script*) gives its flows; `CONTRIBUTING.md` how to run it.
//!
//! - [`build`]: each side built from its own tree, in its own development shell.
//! - [`scenario`]: the scenarios, and one run of one of them: Yuna on the manual clock, driven by
//!   the script's MCP client ([`mcp`]) and its lockstep client ([`lockstep`]), natively or under
//!   valgrind ([`valgrind`]).
//! - [`operations`]: the layout's and motion's operations, counted by callgrind.
//! - [`figures`]: the figures from the runs, their classes and thresholds; [`rule`]: the verdicts.
//! - [`baseline`]: the pinned baseline's file; [`record`]: the run's file; [`report`]: its
//!   Markdown.
//!
//! The runs are drawn in a random order from a recorded seed and run several at a time: what
//! valgrind counts does not depend on what else runs.

pub mod baseline;
pub mod build;
pub mod figures;
pub mod launch;
pub mod lockstep;
pub mod mcp;
pub mod operations;
pub mod record;
pub mod report;
pub mod rule;
pub mod scenario;
pub mod thresholds;
pub mod toml;
pub mod valgrind;

use std::collections::{BTreeMap, VecDeque};
use std::fs;
use std::io::Write as _;
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use build::{Built, Canary, Source};
use figures::{Finished, Job, Side, What};
use record::{FigureRecord, RunFile, RunRecord};
use scenario::{Build, Instrument, SCENARIOS, Tools};
use valgrind::Tool;

const HELP: &str = "\
yuna-bench parity: two builds of Yuna compared on figures the machine's load does not move
(ADR-0142): callgrind's instructions, DHAT's allocations, memory, wakes, the layout's operations.

  yuna-bench parity run [options]

Options:
  --base <revision>|baseline  the base: a revision, or the baseline the head's
                              crates/yuna-bench/parity-baseline.toml pins (default: baseline)
  --base-path <folder>        the base: a folder's tree, built as it is
  --head <revision>           the head (default: HEAD)
  --head-path <folder>        the head: a folder's tree, built as it is
  --tier ci                   the figures CI compares at each push (the only tier so far)
  --before <revision>         the push before the head's: the moves of the baseline its file
                              lacks are the head's own, and a figure worse is compared with it
                              too, to say whether this push moved it. CI passes the push before,
                              a pull request's base (HEAD^1), or the head itself for the
                              schedule; by hand, needed against the pinned baseline (empty or
                              zeros: none)
  --canary <kind>             build the head with an injected cost, which must be flagged:
                              spin, spin-total, allocate, leak, memory, wake, frame-wake,
                              round, layout, motion
  --scenario <list>           idle, partial, full, motion, scaled, many, input, operations
                              (default: all; idle is always run, the others subtract it)
  --repeat <n>                measure each figure n times on each side (default: 1)
  --jobs <n>                  runs at once (default: the processors, 4 at most)
  --seed <n>                  the runs' order (default: from the clock); recorded
  --work <folder>             the builds, the runs' files and sockets, never under /tmp
                              (default: target/parity/work-<pid>-<time>)
  --out <file>                the run file (default: target/parity/parity-<time>.json); its
                              report, in Markdown, beside it
  --keep                      keep the work folder, which a failed or void run keeps anyway

Exit status: 0 no figure worse; 1 a figure worse; 2 a figure void, or an error.
Run it in the parity shell: nix develop .#parity -c cargo run --release -p yuna-bench -- parity run
";

/// What `parity run` asks.
#[derive(Debug, Clone, PartialEq)]
pub struct Options {
    /// The base; `None` for the pinned baseline.
    pub base: Option<Source>,
    /// The head.
    pub head: Source,
    /// The canary the head is built with.
    pub canary: Option<Canary>,
    /// The push before the head's: the moves of the baseline its file lacks are the head's own;
    /// compared with the head on what flags, it says which push moved it.
    pub before: Option<String>,
    /// The scenarios, and `operations`.
    pub scenarios: Vec<String>,
    /// Measurements of each figure on each side.
    pub repeat: u32,
    /// Runs at once.
    pub jobs: usize,
    /// The order's seed.
    pub seed: Option<u64>,
    /// The work folder.
    pub work: Option<PathBuf>,
    /// The run file.
    pub out: Option<PathBuf>,
    /// Whether to keep the work folder.
    pub keep: bool,
}

/// The scenarios' names, and `operations`.
fn every_part() -> Vec<String> {
    SCENARIOS
        .iter()
        .map(|s| s.name.to_owned())
        .chain(["operations".to_owned()])
        .collect()
}

/// Reads `parity run`'s options.
pub fn read_options(arguments: &[String]) -> Result<Options, String> {
    let mut options = Options {
        base: None,
        head: Source::Revision("HEAD".to_owned()),
        canary: None,
        before: None,
        scenarios: every_part(),
        repeat: 1,
        jobs: std::thread::available_parallelism().map_or(1, |n| n.get().min(4)),
        seed: None,
        work: None,
        out: None,
        keep: false,
    };
    let mut base_given = false;
    let mut head_given = false;
    let mut arguments = arguments.iter();
    while let Some(argument) = arguments.next() {
        let mut value = || {
            arguments
                .next()
                .cloned()
                .ok_or_else(|| format!("{argument} needs a value"))
        };
        let once = |given: &mut bool| {
            if std::mem::replace(given, true) {
                Err("give the base, and the head, once each".to_owned())
            } else {
                Ok(())
            }
        };
        match argument.as_str() {
            "--base" => {
                once(&mut base_given)?;
                let revision = value()?;
                options.base = (revision != "baseline").then_some(Source::Revision(revision));
            }
            "--base-path" => {
                once(&mut base_given)?;
                options.base = Some(Source::Path(PathBuf::from(value()?)));
            }
            "--head" => {
                once(&mut head_given)?;
                options.head = Source::Revision(value()?);
            }
            "--head-path" => {
                once(&mut head_given)?;
                options.head = Source::Path(PathBuf::from(value()?));
            }
            "--tier" => {
                let tier = value()?;
                if tier != "ci" {
                    return Err(format!(
                        "no tier {tier:?}: the CI tier, `ci`, is the only one so far"
                    ));
                }
            }
            "--before" => {
                // Empty or zeros, as GitHub gives a push that makes a branch: none.
                let before = value()?;
                options.before =
                    (!before.is_empty() && before.bytes().any(|b| b != b'0')).then_some(before);
            }
            "--canary" => {
                let name = value()?;
                options.canary =
                    Some(Canary::named(&name).ok_or_else(|| format!("no canary {name:?}"))?);
            }
            "--scenario" => {
                let parts = every_part();
                options.scenarios = value()?
                    .split(',')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(|name| {
                        parts
                            .iter()
                            .find(|part| *part == name)
                            .cloned()
                            .ok_or_else(|| format!("no scenario {name:?}"))
                    })
                    .collect::<Result<_, _>>()?;
                if !options.scenarios.iter().any(|s| s == "idle")
                    && options.scenarios.iter().any(|s| s != "operations")
                {
                    options.scenarios.insert(0, "idle".to_owned());
                }
            }
            "--repeat" => {
                options.repeat = value()?
                    .parse()
                    .ok()
                    .filter(|&n| n > 0)
                    .ok_or("--repeat takes a whole number above 0")?;
            }
            "--jobs" => {
                options.jobs = value()?
                    .parse()
                    .ok()
                    .filter(|&n| n > 0)
                    .ok_or("--jobs takes a whole number above 0")?;
            }
            "--seed" => options.seed = Some(value()?.parse().map_err(|_| "--seed takes a number")?),
            "--work" => options.work = Some(PathBuf::from(value()?)),
            "--out" => options.out = Some(PathBuf::from(value()?)),
            "--keep" => options.keep = true,
            other => return Err(format!("unknown argument {other:?}")),
        }
    }
    Ok(options)
}

/// Every run the options ask for, on both sides, in a random order from `seed`; the operations'
/// cases are those each side's programs list.
pub fn plan(options: &Options, built: &[&Built; 2], seed: u64) -> Vec<Job> {
    let mut jobs = plan_side(Side::Base, built[0], &options.scenarios, options.repeat);
    jobs.extend(plan_side(
        Side::Head,
        built[1],
        &options.scenarios,
        options.repeat,
    ));
    crate::cli::shuffle(&mut jobs, seed);
    jobs
}

/// The runs of `side`, built as `built`, for `parts` (scenarios, and `operations`), each
/// measured `repeat` times; none for a side that failed to build.
fn plan_side(side: Side, built: &Built, parts: &[String], repeat: u32) -> Vec<Job> {
    let mut jobs = Vec::new();
    if built.info.error.is_some() {
        return jobs;
    }
    for repeat in 1..=repeat {
        for scenario in SCENARIOS
            .iter()
            .filter(|s| parts.iter().any(|name| name == s.name))
        {
            let n = scenario.periods;
            for (instrument, periods) in [
                (Instrument::Native, n),
                (Instrument::Valgrind(Tool::Callgrind), n),
                (Instrument::Valgrind(Tool::Callgrind), 2 * n),
                (Instrument::Valgrind(Tool::Dhat), n),
                (Instrument::Valgrind(Tool::Dhat), 2 * n),
            ] {
                jobs.push(Job {
                    side,
                    what: What::Scenario {
                        scenario,
                        instrument,
                        periods,
                    },
                    repeat,
                });
            }
        }
        if !parts.iter().any(|s| s == "operations") {
            continue;
        }
        // A program whose cases could not be listed runs nothing: its figure is void
        // (`figures.rs`).
        for (program, cases) in &built.cases {
            let Ok(cases) = cases else { continue };
            let repeats = operations::repeats(program);
            for case in cases {
                for count in [repeats, 2 * repeats] {
                    jobs.push(Job {
                        side,
                        what: What::Operation {
                            program: program.clone(),
                            case: case.clone(),
                            repeats: count,
                        },
                        repeat,
                    });
                }
            }
        }
    }
    jobs
}

/// Runs `yuna-bench parity <arguments>`.
pub fn main(arguments: &[String]) -> ExitCode {
    match arguments.first().map(String::as_str) {
        Some("run") => match run(&arguments[1..]) {
            Ok(status) => ExitCode::from(status),
            Err(error) => {
                eprintln!("yuna-bench parity: {error}");
                ExitCode::from(2)
            }
        },
        // The first process of each run's namespaces, which the script starts itself.
        Some("launch") => launch::main(&arguments[1..]),
        Some("-h" | "--help") | None => {
            print!("{HELP}");
            ExitCode::SUCCESS
        }
        Some(other) => {
            eprintln!("yuna-bench parity: unknown command {other:?}; see --help");
            ExitCode::from(2)
        }
    }
}

/// `parity run`: reads the baseline files, builds both sides, makes every run, judges every
/// figure, writes the run file and its report; gives the exit status.
fn run(arguments: &[String]) -> Result<u8, String> {
    // Before any thread or child: no descriptor this process inherited, `run.sh --exclusive`'s
    // lock or cargo's jobserver, may reach a build or a Yuna.
    let inherited = crate::sandbox::close_inherited_on_exec();
    let options = read_options(arguments)?;
    let repository = crate::scenarios::workspace();
    // Before any build, so that a run that cannot tell which moves are new fails at once.
    let head_file =
        baseline::at(&options.head, &repository).map_err(|e| format!("the head's {e}"))?;
    let base = match &options.base {
        Some(source) => source.clone(),
        None => Source::Revision(
            head_file
                .as_ref()
                .ok_or_else(|| {
                    format!(
                        "the head has no {}, which pins its baseline: name the base with --base",
                        baseline::PATH
                    )
                })?
                .commit
                .clone(),
        ),
    };
    let before = options
        .before
        .as_deref()
        .map(|revision| PushBefore::read(revision, &repository))
        .transpose()?;
    let moves = new_moves(head_file.as_ref(), options.base.is_none(), before.as_ref())?;
    let valgrind = find_on_path("valgrind")
        .ok_or("no valgrind on PATH: run the script in the parity shell, nix develop .#parity")?;
    // The shell's util-linux, never the host's: it makes each run's namespaces.
    let unshare = std::env::var_os("YUNA_UNSHARE")
        .map(PathBuf::from)
        .ok_or("no YUNA_UNSHARE: run the script in the parity shell, nix develop .#parity")?;
    let started = SystemTime::now();
    let started_s = started
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let seed = options.seed.unwrap_or_else(|| {
        started
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos() as u64)
    });
    let work = options.work.clone().unwrap_or_else(|| {
        repository.join(format!(
            "target/parity/work-{}-{started_s}",
            std::process::id()
        ))
    });
    let (work, runtime) = make_work_folder(&work)?;
    let out = options.out.clone().unwrap_or_else(|| {
        repository.join(format!(
            "target/parity/parity-{}.json",
            crate::cli::utc(started).replace([':', '-'], "")
        ))
    });
    if let Some(folder) = out.parent() {
        fs::create_dir_all(folder).map_err(|e| format!("{}: {e}", folder.display()))?;
    }
    let tools = Tools {
        program: std::env::current_exe().map_err(|e| format!("this program: {e}"))?,
        valgrind,
        runtime: runtime.clone(),
        unshare,
    };
    let mut file = RunFile {
        format: record::FORMAT,
        script_commit: crate::scenarios::commit(&[]),
        started_utc: crate::cli::utc(started),
        arguments: arguments.to_vec(),
        tier: "ci".to_owned(),
        seed,
        machine: crate::machine::MachineInfo::read("", None),
        valgrind: Command::new(&tools.valgrind)
            .arg("--version")
            .output()
            .ok()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned()),
        ..RunFile::default()
    };
    if !inherited.is_empty() {
        file.notes.push(format!(
            "descriptors inherited and kept from every child: {inherited:?}"
        ));
    }
    if options.base.is_some()
        && let Some(old) = &moves.old
    {
        file.notes.push(format!(
            "the head moves the baseline from {}: that move is checked only against the pinned \
             baseline, without --base",
            short(old)
        ));
    }
    let runs = Runs {
        options: &options,
        repository: &repository,
        work: &work,
        tools: &tools,
        out: &out,
        seed,
    };
    // From the first build on, whatever ends the run: an error, a panic, or its end.
    let worktrees = Worktrees {
        repository: &repository,
        work: &work,
    };

    eprintln!("yuna-bench parity: building the base");
    let base_built = runs.build(Side::Base, &base, None, &mut file);
    eprintln!("yuna-bench parity: building the head");
    let head_built = runs.build(Side::Head, &options.head, options.canary, &mut file);
    for side in [&base_built, &head_built] {
        if let Some(error) = &side.info.error {
            eprintln!(
                "yuna-bench parity: the {} failed to build: {error}",
                side.info.side
            );
        }
    }
    file.notes
        .extend(side_notes(&base_built, &head_built, options.canary));
    let (head_thresholds, base_thresholds) =
        thresholds_of(&repository, &base_built, &head_built, &mut file.notes)?;
    let judging = figures::Judging {
        head: &head_thresholds,
        base: base_thresholds.as_ref(),
        moves: &moves.figures,
    };
    let built = [&base_built, &head_built];
    let jobs = plan(&options, &built, seed);
    file.order = jobs.iter().map(Job::label).collect();
    record::write(&out, &mut file)?;

    let sides = BTreeMap::from([(Side::Base, &base_built), (Side::Head, &head_built)]);
    let mut finished = run_jobs(&jobs, &sides, &tools, &work, options.jobs, &out, &mut file)?;
    let judged = figures::judge_all(&finished, &built, judging);
    let failed_build = base_built.info.error.is_some() || head_built.info.error.is_some();
    if options.base.is_none()
        && !failed_build
        && let Some(old) = &moves.old
    {
        let check = check_move(
            old,
            &moves,
            &head_built,
            &head_thresholds,
            &runs,
            &mut finished,
            &mut file,
        )?;
        file.move_check = Some(check);
    }
    let worse: Vec<&figures::Judged> = judged
        .iter()
        .filter(|figure| figure.verdict == rule::Verdict::Worse)
        .collect();
    if let Some(before) = &before
        && !worse.is_empty()
    {
        file.pushes = which_push(
            before,
            &worse,
            &head_built,
            judging,
            &runs,
            &mut finished,
            &mut file,
        )?;
    }

    file.figures = judged.iter().map(FigureRecord::of).collect();
    let status = if failed_build {
        2
    } else {
        exit_status(&judged, file.move_check.as_ref())
    };
    file.exit = Some(status);
    record::write(&out, &mut file)?;
    let summary = write_report(&out, &file)?;

    drop(worktrees);
    let _ = fs::remove_dir_all(&runtime);
    let clean = status != 2 && finished.iter().all(|run| run.outcome.error.is_none());
    if options.keep || !clean {
        eprintln!(
            "yuna-bench parity: the builds and the runs' files are kept in {}",
            work.display()
        );
    } else if options.work.is_none() {
        let _ = fs::remove_dir_all(&work);
    }
    println!("{}", out.display());
    println!("{}", summary.display());
    Ok(status)
}

/// Makes the work folder `asked` and its folder of sockets, `rt`, empty; gives both, the work
/// folder's path made absolute.
fn make_work_folder(asked: &Path) -> Result<(PathBuf, PathBuf), String> {
    fs::create_dir_all(asked).map_err(|e| format!("{}: {e}", asked.display()))?;
    let work = fs::canonicalize(asked).map_err(|e| format!("{}: {e}", asked.display()))?;
    // Each run has a /tmp of its own (`launch.rs`), which hides the one the script sees.
    if work.starts_with("/tmp") {
        return Err(format!(
            "the work folder {} is under /tmp, which each run replaces with its own: give \
             --work another folder",
            work.display()
        ));
    }
    // Yuna's seat's socket in it must stay under Unix's 108 bytes.
    let runtime = work.join("rt");
    if runtime.as_os_str().len() > 64 {
        return Err(format!(
            "the work folder's path is too long for its sockets: {} (64 bytes at most for \
             <work>/rt)",
            runtime.display()
        ));
    }
    let _ = fs::remove_dir_all(&runtime);
    fs::DirBuilder::new()
        .mode(0o700)
        .create(&runtime)
        .map_err(|e| format!("{}: {e}", runtime.display()))?;
    Ok((work, runtime))
}

/// The exit status of a run whose builds both built: its figures' verdicts, and a move that does
/// not name every figure it makes worse than the old baseline, which makes the run void.
fn exit_status(judged: &[figures::Judged], move_check: Option<&record::MoveCheck>) -> u8 {
    let unmoved = move_check
        .filter(|check| !check.unnamed.is_empty())
        .map(|check| {
            rule::Verdict::Void(format!(
                "the move from {} does not name every figure it makes worse",
                short(&check.old)
            ))
        });
    rule::exit_status(judged.iter().map(|figure| &figure.verdict).chain(&unmoved))
}

/// Writes the report of `file`, in Markdown, beside the run file `out`, and appends it to the CI
/// job's summary when there is one; gives the report's path.
fn write_report(out: &Path, file: &RunFile) -> Result<PathBuf, String> {
    let text = report::markdown(file);
    let summary = out.with_extension("md");
    crate::cli::write_atomically(&summary, |writer| writer.write_all(text.as_bytes()))?;
    if let Some(step_summary) = std::env::var_os("GITHUB_STEP_SUMMARY") {
        let appended = fs::OpenOptions::new()
            .append(true)
            .create(true)
            .open(&step_summary)
            .and_then(|mut summary| summary.write_all(text.as_bytes()));
        if let Err(error) = appended {
            eprintln!("yuna-bench parity: GITHUB_STEP_SUMMARY: {error}");
        }
    }
    Ok(summary)
}

/// What a run's later builds and runs share: the options, the folders, the tools, the run file's
/// path and the order's seed.
struct Runs<'a> {
    options: &'a Options,
    repository: &'a Path,
    work: &'a Path,
    tools: &'a Tools,
    out: &'a Path,
    seed: u64,
}

impl Runs<'_> {
    /// Builds `side` from `source`, `canary` built in, in the work folder's `<side>`, and records
    /// it in `file`.
    fn build(
        &self,
        side: Side,
        source: &Source,
        canary: Option<Canary>,
        file: &mut RunFile,
    ) -> Built {
        let built = build::build(
            side.name(),
            source,
            self.repository,
            &self.work.join(side.name()),
            canary,
        );
        file.sides.push(built.info.clone());
        built
    }

    /// Makes the runs of `parts` on `side`, built as `built`, in a random order from the seed.
    fn measure(
        &self,
        side: Side,
        built: &Built,
        parts: &[String],
        file: &mut RunFile,
    ) -> Result<Vec<Finished>, String> {
        let mut jobs = plan_side(side, built, parts, self.options.repeat);
        crate::cli::shuffle(&mut jobs, self.seed);
        run_jobs(
            &jobs,
            &BTreeMap::from([(side, built)]),
            self.tools,
            self.work,
            self.options.jobs,
            self.out,
            file,
        )
    }
}

/// Removes the revisions' worktrees from the work folder once dropped: at the end of a run,
/// whether it ended, failed or panicked. A side built from a folder has none.
struct Worktrees<'a> {
    repository: &'a Path,
    work: &'a Path,
}

impl Drop for Worktrees<'_> {
    fn drop(&mut self) {
        for side in [Side::Base, Side::Head, Side::Old, Side::Before] {
            build::remove_tree(self.repository, &self.work.join(side.name()));
        }
    }
}

/// What a reader needs to know of the two sides: a self control, a canary.
fn side_notes(base: &Built, head: &Built, canary: Option<Canary>) -> Vec<String> {
    let mut notes = Vec::new();
    let same_tree = base.info.commit.is_some()
        && base.info.commit == head.info.commit
        && base.info.dirty == head.info.dirty
        && base.info.asked == head.info.asked;
    if same_tree && canary.is_none() {
        let code = if base.info.code == head.info.code {
            "the same code (`.text`)"
        } else {
            "different code (`.text`): see each program's digests"
        };
        let files = if base.info.programs == head.info.programs {
            "the same files, byte for byte"
        } else {
            "files that differ elsewhere: two trees' run paths, which the shell's linker wrapper \
             writes into each"
        };
        notes.push(format!(
            "a self control: the same revision built twice, in two folders, gives {code} and \
             {files}"
        ));
    }
    if let Some(canary) = canary {
        notes.push(format!(
            "a control: the head is built with the canary `{}`, which must flag its figure",
            canary.name()
        ));
    }
    notes
}

/// The thresholds that judge, the head's, and those the head may not loosen, the base's. A head
/// without the file, a revision from before it, is judged by the script's own tree's; a base
/// without it checks nothing; a file that does not read, or lacks a kind, fails the run.
fn thresholds_of(
    repository: &Path,
    base: &Built,
    head: &Built,
    notes: &mut Vec<String>,
) -> Result<(thresholds::Thresholds, Option<thresholds::Thresholds>), String> {
    let head_thresholds = match &head.thresholds {
        Some(read) => read.clone().map_err(|e| format!("the head's {e}"))?,
        None => {
            notes.push(format!(
                "the head's tree has no {}: the script's own judges it",
                thresholds::PATH
            ));
            let path = repository.join(thresholds::PATH);
            let text = fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
            thresholds::parse(&text).map_err(|e| format!("{}: {e}", path.display()))?
        }
    };
    head_thresholds
        .check(&figures::kind_names())
        .map_err(|e| format!("{}: {e}", thresholds::PATH))?;
    let base_thresholds = match &base.thresholds {
        Some(read) => Some(read.clone().map_err(|e| format!("the base's {e}"))?),
        None => {
            notes.push(format!(
                "the base's tree has no {}: no threshold is checked against it",
                thresholds::PATH
            ));
            None
        }
    };
    Ok((head_thresholds, base_thresholds))
}

/// The push before the head's, `--before`: its commit and its baseline file.
#[derive(Debug, Clone, PartialEq)]
struct PushBefore {
    /// Its commit, in full.
    commit: String,
    /// Its tree's baseline file; none if its tree has none.
    baseline: Option<baseline::Baseline>,
}

impl PushBefore {
    /// Whether it is the tree `head` was built from: a scheduled run's push before is its head,
    /// the same code, nothing to compare.
    fn is(&self, head: &build::SideInfo) -> bool {
        head.commit.as_deref() == Some(self.commit.as_str()) && head.dirty == Some(false)
    }

    /// The push before named `revision` in `repository`, read.
    fn read(revision: &str, repository: &Path) -> Result<PushBefore, String> {
        let commit = build::commit_of(repository, revision)
            .map_err(|e| format!("the push before, {revision}: {e}"))?;
        let baseline = baseline::at(&Source::Revision(commit.clone()), repository)
            .map_err(|e| format!("the push before's {e}"))?;
        Ok(PushBefore { commit, baseline })
    }
}

/// The moves of the baseline that are this push's own, and the baseline before them.
#[derive(Debug, Clone, Default, PartialEq)]
struct NewMoves {
    /// The figures each new move names: what the head may make worse than the old baseline, or
    /// judge more loosely than the base and the old baseline do.
    figures: Vec<Vec<String>>,
    /// The old baseline, which the push before pinned, to run the head against at the push that
    /// moves it; none when nothing moved, or for the first baseline, the first move, made where
    /// the push before had no baseline file: it has no old one.
    old: Option<String>,
}

/// The moves the head's baseline file holds that the push before's lacks.
///
/// "New" is judged against the push before, never the base: a baseline's own file cannot hold
/// the move to itself. So a later push, a pull request whose base already moved, and a scheduled
/// run, whose push before is its head, bring none. Against the pinned baseline (`base_is_pinned`)
/// without the push before, by hand, the run asks for `--before`; against a base `--base` names,
/// it takes none as new.
///
/// The head's moves must be the push before's with new ones at the end: a move once pushed is
/// never changed or dropped, the first baseline included. A new move may not pin the commit the
/// move before it pinned: it would be checked against itself.
fn new_moves(
    head: Option<&baseline::Baseline>,
    base_is_pinned: bool,
    before: Option<&PushBefore>,
) -> Result<NewMoves, String> {
    let Some(head) = head else {
        return Ok(NewMoves::default());
    };
    let Some(before) = before else {
        if !base_is_pinned {
            return Ok(NewMoves::default());
        }
        return Err(format!(
            "the head's {} pins the baseline, whose own file cannot hold the move to it: give \
             --before, the revision the head's work starts from (origin/main for commits not \
             pushed yet, HEAD for a tree's changes that move nothing), so that the script knows \
             which moves are the head's own",
            baseline::PATH
        ));
    };
    let known = before.baseline.as_ref().map_or(&[][..], |b| &b.moves);
    if !head.moves.starts_with(known) {
        return Err(format!(
            "the head's {} changes or drops a move that the push before, {}, held: a move is only \
             ever added, at the end",
            baseline::PATH,
            short(&before.commit)
        ));
    }
    if let Some(again) = (known.len().max(1)..head.moves.len())
        .find(|&at| head.moves[at].commit == head.moves[at - 1].commit)
    {
        return Err(format!(
            "the head's {} moves the baseline to {}, which the move before already pinned: a move \
             is checked once, at the push that makes it, and this one would be checked against \
             itself. A move check that failed or was cancelled is run again by hand, with \
             --before the commit before that move (CONTRIBUTING.md)",
            baseline::PATH,
            short(&head.moves[again].commit)
        ));
    }
    let fresh = &head.moves[known.len()..];
    let figures = fresh.iter().map(|m| m.figures.clone()).collect();
    if fresh.is_empty() {
        return Ok(NewMoves { figures, old: None });
    }
    match &before.baseline {
        Some(pinned) => Ok(NewMoves {
            figures,
            old: Some(pinned.commit.clone()),
        }),
        // The first baseline, where the push before had no file: no old one to check it against.
        None if head.moves.len() == 1 => Ok(NewMoves { figures, old: None }),
        None => Err(format!(
            "the push before, {}, has no {}, and the head's holds moves after the first \
             baseline, which cannot be checked: give --before a revision that has it",
            short(&before.commit),
            baseline::PATH
        )),
    }
}

/// The check of the baseline's move, at the push that makes it: the head run against the old
/// baseline, `old`, where a figure may be worse, or judged more loosely than the old baseline's
/// file judges it, only if a new move names it.
fn check_move(
    old: &str,
    moves: &NewMoves,
    head: &Built,
    head_thresholds: &thresholds::Thresholds,
    runs: &Runs,
    finished: &mut Vec<Finished>,
    file: &mut RunFile,
) -> Result<record::MoveCheck, String> {
    eprintln!(
        "yuna-bench parity: building the old baseline, {}",
        short(old)
    );
    let old_built = runs.build(Side::Old, &Source::Revision(old.to_owned()), None, file);
    let mut check = record::MoveCheck {
        old: old.to_owned(),
        named: moves.figures.iter().flatten().cloned().collect(),
        ..record::MoveCheck::default()
    };
    if let Some(error) = &old_built.info.error {
        check
            .unnamed
            .push(format!("the old baseline did not build: {error}"));
        return Ok(check);
    }
    let old_thresholds = old_built
        .thresholds
        .clone()
        .transpose()
        .map_err(|e| format!("the old baseline's {e}"))?;
    finished.extend(runs.measure(Side::Old, &old_built, &runs.options.scenarios, file)?);
    let against_old = figures::judge_all(
        &figures::against(finished, Side::Old),
        &[&old_built, head],
        figures::Judging {
            head: head_thresholds,
            base: old_thresholds.as_ref(),
            moves: &moves.figures,
        },
    );
    for figure in &against_old {
        let named = check
            .named
            .iter()
            .any(|n| *n == figure.name || n == figure.kind.name);
        match &figure.verdict {
            rule::Verdict::Worse if named => check.worse.push(figure.name.clone()),
            rule::Verdict::Worse => check.unnamed.push(figure.name.clone()),
            rule::Verdict::Void(reason) => {
                check
                    .unnamed
                    .push(format!("{}: void against it: {reason}", figure.name));
            }
            _ => {}
        }
    }
    Ok(check)
}

/// Which push moved each figure of `worse`: the push before run on their scenarios and judged
/// against the head, as `judging` judged the base. It says; it never changes a verdict.
fn which_push(
    before: &PushBefore,
    worse: &[&figures::Judged],
    head: &Built,
    judging: figures::Judging,
    runs: &Runs,
    finished: &mut Vec<Finished>,
    file: &mut RunFile,
) -> Result<Vec<record::PushNote>, String> {
    let note = |figure: &figures::Judged, said: String| record::PushNote {
        figure: figure.name.clone(),
        said,
    };
    if before.is(&head.info) {
        let said = "not compared: the push before is the head's own commit";
        return Ok(worse.iter().map(|f| note(f, said.to_owned())).collect());
    }
    eprintln!(
        "yuna-bench parity: building the push before, {}",
        short(&before.commit)
    );
    let before_built = runs.build(
        Side::Before,
        &Source::Revision(before.commit.clone()),
        None,
        file,
    );
    let mut parts: Vec<String> = worse
        .iter()
        .map(|figure| match figure.group.as_str() {
            "operations" | "motion_operations" => "operations".to_owned(),
            scenario => scenario.to_owned(),
        })
        .collect();
    parts.push("idle".to_owned());
    parts.sort();
    parts.dedup();
    let against_before = if before_built.info.error.is_none() {
        finished.extend(runs.measure(Side::Before, &before_built, &parts, file)?);
        figures::judge_all(
            &figures::against(finished, Side::Before),
            &[&before_built, head],
            judging,
        )
    } else {
        Vec::new()
    };
    Ok(worse
        .iter()
        .map(|figure| {
            let then = against_before.iter().find(|f| f.name == figure.name);
            let said = match then.map(|f| &f.verdict) {
                Some(rule::Verdict::Worse) => "this push made it worse".to_owned(),
                Some(rule::Verdict::NoWorse) => {
                    "it was as bad at the push before: an earlier push made it worse".to_owned()
                }
                Some(rule::Verdict::Void(reason)) => format!("unknown: {reason}"),
                Some(rule::Verdict::Recorded) | None => before_built
                    .info
                    .error
                    .as_ref()
                    .map_or("unknown: not measured".to_owned(), |e| {
                        format!("unknown: the push before did not build: {e}")
                    }),
            };
            note(figure, said)
        })
        .collect())
}

/// A commit's first 12 digits.
fn short(commit: &str) -> &str {
    &commit[..commit.len().min(12)]
}

/// Makes every run of `jobs`, `parallel` at a time, writing the run file after each.
fn run_jobs(
    jobs: &[Job],
    built: &BTreeMap<Side, &Built>,
    tools: &Tools,
    work: &Path,
    parallel: usize,
    out: &Path,
    file: &mut RunFile,
) -> Result<Vec<Finished>, String> {
    let builds: BTreeMap<Side, Build> = built
        .iter()
        .map(|(side, built)| {
            let build = Build {
                yuna: built.programs.get("yuna").cloned().unwrap_or_default(),
                environment: built.run_environment.clone(),
                // Read by Yuna's own canaries alone: the layout's and motion's are built in,
                // unasked.
                canary: built
                    .info
                    .canary
                    .as_deref()
                    .and_then(Canary::named)
                    .filter(|canary| canary.package() == "yuna")
                    .map(|canary| canary.name().to_owned()),
            };
            (*side, build)
        })
        .collect();
    let queue = Mutex::new(jobs.iter().cloned().enumerate().collect::<VecDeque<_>>());
    let done = Mutex::new((Vec::<Finished>::new(), file));
    let total = jobs.len();
    let write_error = Mutex::new(None);
    std::thread::scope(|scope| {
        for _ in 0..parallel.max(1) {
            scope.spawn(|| {
                loop {
                    let Some((index, job)) = queue.lock().expect("the queue").pop_front() else {
                        break;
                    };
                    let label = job.label();
                    let folder = work.join("runs").join(label.replace('/', "_"));
                    // Empty, so that no file of an earlier run in the same work folder is summed
                    // with this one's.
                    let _ = fs::remove_dir_all(&folder);
                    let side = job.side;
                    let outcome = match &job.what {
                        What::Scenario {
                            scenario,
                            instrument,
                            periods,
                        } => scenario::run(
                            scenario,
                            *instrument,
                            *periods,
                            &builds[&side],
                            tools,
                            &folder,
                            index,
                        ),
                        What::Operation {
                            program,
                            case,
                            repeats,
                        } => match built[&side].programs.get(program) {
                            Some(path) => {
                                operations::run(path, &tools.valgrind, case, *repeats, &folder)
                            }
                            None => scenario::Outcome {
                                error: Some(format!("no {program}")),
                                ..scenario::Outcome::default()
                            },
                        },
                    };
                    let finished = Finished { job, outcome };
                    let mut done = done.lock().expect("the runs made");
                    let (runs, file) = &mut *done;
                    eprintln!(
                        "yuna-bench parity: [{}/{total}] {label}: {:.1} s{}",
                        runs.len() + 1,
                        finished.outcome.seconds,
                        finished
                            .outcome
                            .error
                            .as_deref()
                            .map(|e| format!(", failed: {e}"))
                            .unwrap_or_default()
                    );
                    file.runs.push(RunRecord::of(&finished));
                    runs.push(finished);
                    if let Err(error) = record::write(out, file) {
                        *write_error.lock().expect("the write's error") = Some(error);
                    }
                }
            });
        }
    });
    if let Some(error) = write_error.into_inner().expect("the write's error") {
        return Err(error);
    }
    Ok(done.into_inner().expect("the runs made").0)
}

/// `name`'s path on this process's `PATH`.
fn find_on_path(name: &str) -> Option<PathBuf> {
    std::env::var_os("PATH")?
        .to_str()?
        .split(':')
        .map(|folder| Path::new(folder).join(name))
        .find(|candidate| candidate.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn read(arguments: &[&str]) -> Result<Options, String> {
        read_options(
            &arguments
                .iter()
                .map(|a| (*a).to_owned())
                .collect::<Vec<_>>(),
        )
    }

    #[test]
    fn options_default_to_the_baseline_against_head_every_scenario_once() {
        let options = read(&[]).unwrap();
        assert_eq!(options.base, None);
        assert_eq!(options.head, Source::Revision("HEAD".to_owned()));
        assert_eq!(options.scenarios.len(), 8);
        assert_eq!(options.repeat, 1);
        let options = read(&[
            "--base",
            "2e14b86",
            "--head-path",
            ".",
            "--tier",
            "ci",
            "--canary",
            "spin",
            "--scenario",
            "partial",
        ])
        .unwrap();
        assert_eq!(options.base, Some(Source::Revision("2e14b86".to_owned())));
        assert_eq!(options.head, Source::Path(PathBuf::from(".")));
        assert_eq!(options.canary, Some(Canary::Spin));
        // idle comes with any scenario: the others subtract its allocations.
        assert_eq!(options.scenarios, ["idle", "partial"]);
        assert_eq!(
            read(&["--scenario", "operations"]).unwrap().scenarios,
            ["operations"]
        );
        assert_eq!(read(&["--base", "baseline"]).unwrap().base, None);
        assert!(read(&["--base", "a", "--base-path", "b"]).is_err());
        assert!(read(&["--tier", "timed"]).is_err());
        assert!(read(&["--canary", "slow"]).is_err());
        assert!(read(&["--scenario", "sleep"]).is_err());
        assert!(read(&["--repeat", "0"]).is_err());
        assert!(read(&["--jobs", "0"]).is_err());
        assert!(read(&["--seed"]).is_err());
    }

    fn moved(commit: &str, figures: &[&str]) -> baseline::Move {
        baseline::Move {
            commit: commit.repeat(40),
            figures: figures.iter().map(|f| (*f).to_owned()).collect(),
            reason: "why".to_owned(),
            allowed_by: "ADR-0142".to_owned(),
        }
    }

    /// A baseline file holding `moves`, which pins the last.
    fn pinning(moves: &[&baseline::Move]) -> baseline::Baseline {
        baseline::Baseline {
            commit: moves.last().expect("a move").commit.clone(),
            moves: moves.iter().map(|m| (*m).clone()).collect(),
        }
    }

    /// A push before, at `commit`, holding `file`.
    fn before(commit: &str, file: Option<baseline::Baseline>) -> PushBefore {
        PushBefore {
            commit: commit.repeat(40),
            baseline: file,
        }
    }

    /// Whether the head's thresholds, which loosen the moved figure's, are refused when the new
    /// moves are `moves`.
    fn refuses_its_loosening(moves: &NewMoves) -> bool {
        let entry = |absolute: f64| thresholds::Entry {
            figure: "Pss_Anon".to_owned(),
            group: "*".to_owned(),
            class: rule::Class::Parity,
            threshold: rule::Threshold {
                relative: 0.0,
                absolute,
            },
            basis: "a test's".to_owned(),
        };
        let (was, looser) = (
            thresholds::Thresholds {
                entries: vec![entry(512.0)],
            },
            thresholds::Thresholds {
                entries: vec![entry(2048.0)],
            },
        );
        let figures: Vec<&[String]> = moves.figures.iter().map(Vec::as_slice).collect();
        thresholds::loosened(
            &was,
            &looser,
            "Pss_Anon",
            "partial",
            "partial: Pss_Anon",
            &figures,
        )
        .is_some()
    }

    #[test]
    fn a_move_is_checked_at_the_push_that_makes_it_and_never_after() {
        // The baseline moves from a to c, an earlier commit holding the change; the push of d
        // holds the move. c's own file cannot name c: it still pins a.
        let first = moved("a", &[]);
        let to_c = moved("c", &["partial: Pss_Anon"]);
        let head = pinning(&[&first, &to_c]);
        let base = pinning(&[&first]);
        // The push that moves it, and a pull request that makes the move: the push before, and
        // the pull request's base, still pin a.
        let moving = new_moves(Some(&head), true, Some(&before("b", Some(base.clone())))).unwrap();
        assert_eq!(
            moving,
            NewMoves {
                figures: vec![vec!["partial: Pss_Anon".to_owned()]],
                old: Some("a".repeat(40)),
            }
        );
        assert!(!refuses_its_loosening(&moving), "the move names it");
        // A later push, whose push before holds the move; a pull request whose base already
        // moved; a scheduled run, whose push before is its head: the move stays unchecked, and
        // no threshold of what it moved may loosen.
        for (case, known) in [
            ("a later push", before("d", Some(head.clone()))),
            (
                "a pull request after the move",
                before("e", Some(head.clone())),
            ),
            ("a scheduled run", before("f", Some(head.clone()))),
        ] {
            let moves = new_moves(Some(&head), true, Some(&known)).unwrap();
            assert_eq!(moves, NewMoves::default(), "{case}");
            assert!(refuses_its_loosening(&moves), "{case}");
        }
        // Two moves in one push: both named, checked against what the push before pinned.
        let to_g = moved("g", &["many: Pss_Anon"]);
        let two = pinning(&[&first, &to_c, &to_g]);
        let both = new_moves(Some(&two), true, Some(&before("b", Some(base.clone())))).unwrap();
        assert_eq!(both.figures.len(), 2);
        assert_eq!(both.old, Some("a".repeat(40)));
    }

    #[test]
    fn by_hand_the_pinned_baseline_needs_the_push_before() {
        let first = moved("a", &[]);
        let to_c = moved("c", &["partial: Pss_Anon"]);
        let head = pinning(&[&first, &to_c]);
        // Against the pinned baseline, whose file never holds the move to itself.
        let error = new_moves(Some(&head), true, None).unwrap_err();
        assert!(error.contains("--before"), "{error}");
        // Against a base `--base` names, nothing is new without the push before: no move is
        // checked, and no threshold may loosen.
        let named = new_moves(Some(&head), false, None).unwrap();
        assert_eq!(named, NewMoves::default());
        assert!(refuses_its_loosening(&named));
        // A head without the file holds no move.
        assert_eq!(new_moves(None, true, None), Ok(NewMoves::default()));
    }

    #[test]
    fn a_move_once_pushed_is_never_changed_dropped_or_made_again() {
        let first = moved("a", &[]);
        let to_c = moved("c", &["partial: Pss_Anon"]);
        let known = before("b", Some(pinning(&[&first, &to_c])));
        // A move changed after its push: it would name figures nothing checked.
        let changed = moved("c", &["partial: Pss_Anon", "full: Pss_Anon"]);
        let error = new_moves(Some(&pinning(&[&first, &changed])), true, Some(&known)).unwrap_err();
        assert!(error.contains("changes or drops"), "{error}");
        // A move dropped.
        let dropped = pinning(&[&first, &moved("g", &[])]);
        assert!(new_moves(Some(&dropped), true, Some(&known)).is_err());
        // The first baseline rewritten: the sole move a push before holds is no placeholder.
        let real_first = before("b", Some(pinning(&[&moved("5", &[])])));
        let repinned = pinning(&[&moved("9", &["partial: Pss_Anon"])]);
        let error = new_moves(Some(&repinned), true, Some(&real_first)).unwrap_err();
        assert!(error.contains("changes or drops"), "{error}");
        // So is the placeholder of the parity script's own commit, which 37d925a replaced once.
        let placeholder = before("b", Some(pinning(&[&moved("e", &[])])));
        assert!(new_moves(Some(&pinning(&[&first])), true, Some(&placeholder)).is_err());
        // A second move to the commit the push before pinned, as if to repair a move check that
        // failed: it would be checked against itself.
        let again = moved("c", &["full: Pss_Anon"]);
        let error =
            new_moves(Some(&pinning(&[&first, &to_c, &again])), true, Some(&known)).unwrap_err();
        assert!(error.contains("already pinned"), "{error}");
        // Two moves to one commit in the same push.
        let only_first = before("b", Some(pinning(&[&first])));
        let twice = pinning(&[&first, &to_c, &again]);
        assert!(new_moves(Some(&twice), true, Some(&only_first)).is_err());
    }

    #[test]
    fn the_first_baseline_has_no_old_one_where_the_push_before_had_no_file() {
        let first = moved("a", &[]);
        let to_c = moved("c", &["partial: Pss_Anon"]);
        assert_eq!(
            new_moves(Some(&pinning(&[&first])), true, Some(&before("b", None))),
            Ok(NewMoves {
                figures: vec![Vec::new()],
                old: None,
            })
        );
        // Later moves where the push before had no file: nothing pinned to check them against.
        let error = new_moves(
            Some(&pinning(&[&first, &to_c])),
            true,
            Some(&before("b", None)),
        )
        .unwrap_err();
        assert!(error.contains("cannot be checked"), "{error}");
    }

    #[test]
    fn the_worktrees_are_removed_whatever_ends_the_run() {
        let root = crate::testing::folder("parity-worktrees");
        let repository = root.join("repository");
        let work = root.join("work");
        let git = |arguments: &[&str]| build::git(&root, arguments).expect("git");
        git(&["init", "--quiet", &repository.to_string_lossy()]);
        git(&[
            "-C",
            &repository.to_string_lossy(),
            "-c",
            "user.name=parity",
            "-c",
            "user.email=parity@example.invalid",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "--quiet",
            "--allow-empty",
            "--message",
            "A test's commit",
        ]);
        let check_out = |side: Side| {
            let tree = work.join(side.name()).join("tree");
            build::git(
                &repository,
                &[
                    "worktree",
                    "add",
                    "--detach",
                    &tree.to_string_lossy(),
                    "HEAD",
                ],
            )
            .expect("a worktree");
            tree
        };
        let listed = || {
            build::git(&repository, &["worktree", "list"])
                .unwrap()
                .lines()
                .count()
        };
        // An error after the builds.
        let base = check_out(Side::Base);
        let head = check_out(Side::Head);
        let failed = || -> Result<(), String> {
            let _worktrees = Worktrees {
                repository: &repository,
                work: &work,
            };
            Err("the head's thresholds do not read".to_owned())
        };
        assert!(failed().is_err());
        assert!(!base.exists() && !head.exists());
        assert_eq!(listed(), 1, "the repository's own tree alone");
        // A panic, the old baseline's and the push before's trees too.
        let trees = [Side::Old, Side::Before].map(check_out);
        let panicked = std::panic::catch_unwind(|| {
            let _worktrees = Worktrees {
                repository: &repository,
                work: &work,
            };
            panic!("a run that panicked");
        });
        assert!(panicked.is_err());
        assert!(trees.iter().all(|tree| !tree.exists()));
        assert_eq!(listed(), 1);
    }

    #[test]
    fn a_push_before_that_is_the_head_is_not_built_again() {
        let pushed = before("a", None);
        let head = |commit: &str, dirty: bool| build::SideInfo {
            commit: Some(commit.repeat(40)),
            dirty: Some(dirty),
            ..build::SideInfo::default()
        };
        // A scheduled run: its push before is its head.
        assert!(pushed.is(&head("a", false)));
        // A folder at that commit with changes, another commit, a head with no commit.
        assert!(!pushed.is(&head("a", true)));
        assert!(!pushed.is(&head("b", false)));
        assert!(!pushed.is(&build::SideInfo::default()));
    }

    #[test]
    fn the_push_before_is_read_only_when_given_and_real() {
        assert_eq!(read(&["--before", ""]).unwrap().before, None);
        assert_eq!(read(&["--before", &"0".repeat(40)]).unwrap().before, None);
        assert_eq!(
            read(&["--before", "2e14b86"]).unwrap().before,
            Some("2e14b86".to_owned())
        );
    }

    #[test]
    fn the_plan_runs_each_scenario_five_ways_on_each_side_in_a_seeded_order() {
        let built = Built {
            info: build::SideInfo::default(),
            programs: BTreeMap::from([("yuna".to_owned(), PathBuf::from("/w/yuna"))]),
            cases: BTreeMap::new(),
            thresholds: None,
            run_environment: Vec::new(),
        };
        let options = read(&["--scenario", "partial", "--repeat", "2"]).unwrap();
        let jobs = plan(&options, &[&built, &built], 42);
        // 2 sides × 2 repeats × 2 scenarios (idle and partial) × 5 runs.
        assert_eq!(jobs.len(), 40);
        assert_eq!(plan(&options, &[&built, &built], 42), jobs);
        assert_ne!(plan(&options, &[&built, &built], 43), jobs);
        let labels: std::collections::BTreeSet<String> = jobs.iter().map(Job::label).collect();
        assert_eq!(labels.len(), jobs.len(), "each run's label is its own");
        let longer = 2 * scenario::named("partial").expect("partial").periods;
        assert!(labels.contains(&format!("head/partial/dhat-{longer}/2")));
        // A side that failed to build runs nothing.
        let mut failed = built.clone();
        failed.info.error = Some("cargo failed".to_owned());
        assert_eq!(plan(&options, &[&failed, &built], 42).len(), 20);
    }
}
