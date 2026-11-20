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
  --base <revision>|baseline  the base: a revision, or the pinned baseline of
                              crates/yuna-bench/parity-baseline.toml (default: baseline)
  --base-path <folder>        the base: a folder's tree, built as it is
  --head <revision>           the head (default: HEAD)
  --head-path <folder>        the head: a folder's tree, built as it is
  --tier ci                   the figures CI compares at each push (the only tier so far)
  --before <revision>         the push before the head's: when a figure is worse, compared with
                              the head too, to say whether this push moved it (CI passes
                              github.event.before; empty or zeros: none)
  --canary <kind>             build the head with an injected cost, which must be flagged:
                              spin, allocate, leak, memory, wake, round, layout, motion
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
    /// The push before the head's, compared with the head on what flags, to say which push
    /// moved it.
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
                // CI's `github.event.before`: empty outside a push, zeros for a new branch.
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

/// `parity run`: builds both sides, makes every run, judges every figure, writes the run file
/// and its report; gives the exit status.
fn run(arguments: &[String]) -> Result<u8, String> {
    // Before any thread or child: no descriptor this process inherited, `run.sh --exclusive`'s
    // lock or cargo's jobserver, may reach a build or a Yuna.
    let inherited = crate::sandbox::close_inherited_on_exec();
    let options = read_options(arguments)?;
    let repository = crate::scenarios::workspace();
    let pinned = match &options.base {
        Some(_) => None,
        None => {
            let path = repository.join(baseline::PATH);
            let text = fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
            Some(baseline::parse(&text).map_err(|e| format!("{}: {e}", path.display()))?)
        }
    };
    let base = match (&options.base, &pinned) {
        (Some(source), _) => source.clone(),
        (None, Some(pinned)) => Source::Revision(pinned.commit.clone()),
        (None, None) => unreachable!("the pinned baseline is read when no base is given"),
    };
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
    fs::create_dir_all(&work).map_err(|e| format!("{}: {e}", work.display()))?;
    let work = fs::canonicalize(&work).map_err(|e| format!("{}: {e}", work.display()))?;
    // Each run has a /tmp of its own (`launch.rs`), which hides the one the script sees.
    if work.starts_with("/tmp") {
        return Err(format!(
            "the work folder {} is under /tmp, which each run replaces with its own: give \
             --work another folder",
            work.display()
        ));
    }
    // The sockets' folder, in the work folder: Yuna's seat's socket in it must stay under Unix's
    // 108 bytes.
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
    let out = options.out.clone().unwrap_or_else(|| {
        repository.join(format!(
            "target/parity/parity-{}.json",
            crate::cli::utc(started).replace([':', '-'], "")
        ))
    });
    if let Some(folder) = out.parent() {
        fs::create_dir_all(folder).map_err(|e| format!("{}: {e}", folder.display()))?;
    }
    let mut file = RunFile {
        format: record::FORMAT,
        script_commit: crate::scenarios::commit(&[]),
        started_utc: crate::cli::utc(started),
        arguments: arguments.to_vec(),
        tier: "ci".to_owned(),
        seed,
        machine: crate::machine::MachineInfo::read("", None),
        valgrind: Command::new(&valgrind)
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

    eprintln!("yuna-bench parity: building the base");
    let base_built = build::build("base", &base, &repository, &work.join("base"), None);
    eprintln!("yuna-bench parity: building the head");
    let head_built = build::build(
        "head",
        &options.head,
        &repository,
        &work.join("head"),
        options.canary,
    );
    for side in [&base_built, &head_built] {
        if let Some(error) = &side.info.error {
            eprintln!(
                "yuna-bench parity: the {} failed to build: {error}",
                side.info.side
            );
        }
    }
    file.sides = vec![base_built.info.clone(), head_built.info.clone()];
    file.notes
        .extend(side_notes(&base_built, &head_built, options.canary));
    let (head_thresholds, base_thresholds) =
        thresholds_of(&repository, &base_built, &head_built, &mut file.notes)?;
    let moves = new_moves(&base_built, &head_built);
    let judging = figures::Judging {
        head: &head_thresholds,
        base: base_thresholds.as_ref(),
        moves: &moves,
    };
    let built = [&base_built, &head_built];
    let jobs = plan(&options, &built, seed);
    file.order = jobs.iter().map(Job::label).collect();
    record::write(&out, &mut file)?;

    let tools = Tools {
        valgrind,
        program: std::env::current_exe().map_err(|e| format!("this program: {e}"))?,
        runtime: runtime.clone(),
        unshare,
    };
    let sides = BTreeMap::from([(Side::Base, &base_built), (Side::Head, &head_built)]);
    let mut finished = run_jobs(&jobs, &sides, &tools, &work, options.jobs, &out, &mut file)?;
    let judged = figures::judge_all(&finished, &built, judging);
    let failed_build = base_built.info.error.is_some() || head_built.info.error.is_some();

    // The baseline's move, at the push that makes it: the head against the old baseline flags
    // only what the move names.
    let mut extra_trees = Vec::new();
    if let Some(pinned) = &pinned
        && !failed_build
        && let Some(old) = move_to_check(pinned, &base_built, &options, &repository)
    {
        eprintln!("yuna-bench parity: building the old baseline, {old}");
        let old_built = build::build(
            "old",
            &Source::Revision(old.clone()),
            &repository,
            &work.join("old"),
            None,
        );
        extra_trees.push("old");
        file.sides.push(old_built.info.clone());
        let mut check = record::MoveCheck {
            old: old.clone(),
            named: moves.iter().flatten().cloned().collect(),
            ..record::MoveCheck::default()
        };
        if let Some(error) = &old_built.info.error {
            check
                .unnamed
                .push(format!("the old baseline did not build: {error}"));
        } else {
            let mut jobs = plan_side(Side::Old, &old_built, &options.scenarios, options.repeat);
            crate::cli::shuffle(&mut jobs, seed);
            let sides = BTreeMap::from([(Side::Old, &old_built), (Side::Head, &head_built)]);
            finished.extend(run_jobs(
                &jobs,
                &sides,
                &tools,
                &work,
                options.jobs,
                &out,
                &mut file,
            )?);
            let against_old = figures::judge_all(
                &figures::against(&finished, Side::Old),
                &[&old_built, &head_built],
                figures::Judging {
                    head: &head_thresholds,
                    base: None,
                    moves: &[],
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
        }
        file.move_check = Some(check);
    }

    // Which push moved a worse figure: the push before against the head, on what flagged. It
    // says; it never changes the verdict.
    let worse: Vec<&figures::Judged> = judged
        .iter()
        .filter(|figure| figure.verdict == rule::Verdict::Worse)
        .collect();
    if let Some(before) = &options.before
        && !worse.is_empty()
    {
        eprintln!("yuna-bench parity: building the push before, {before}");
        let before_built = build::build(
            "before",
            &Source::Revision(before.clone()),
            &repository,
            &work.join("before"),
            None,
        );
        extra_trees.push("before");
        file.sides.push(before_built.info.clone());
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
            let mut jobs = plan_side(Side::Before, &before_built, &parts, options.repeat);
            crate::cli::shuffle(&mut jobs, seed);
            let sides = BTreeMap::from([(Side::Before, &before_built), (Side::Head, &head_built)]);
            finished.extend(run_jobs(
                &jobs,
                &sides,
                &tools,
                &work,
                options.jobs,
                &out,
                &mut file,
            )?);
            figures::judge_all(
                &figures::against(&finished, Side::Before),
                &[&before_built, &head_built],
                judging,
            )
        } else {
            Vec::new()
        };
        file.pushes = worse
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
                record::PushNote {
                    figure: figure.name.clone(),
                    said,
                }
            })
            .collect();
    }

    file.figures = judged.iter().map(FigureRecord::of).collect();
    // A figure worse than the old baseline that no new move names makes the run void.
    let unmoved = file
        .move_check
        .as_ref()
        .filter(|check| !check.unnamed.is_empty())
        .map(|check| {
            rule::Verdict::Void(format!(
                "the move from {} does not name every figure it makes worse",
                short(&check.old)
            ))
        });
    let status = if failed_build {
        2
    } else {
        rule::exit_status(judged.iter().map(|figure| &figure.verdict).chain(&unmoved))
    };
    file.exit = Some(status);
    record::write(&out, &mut file)?;
    let text = report::markdown(&file);
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

    for side in ["base", "head"].into_iter().chain(extra_trees) {
        build::remove_tree(&repository, &work.join(side));
    }
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

/// The figures each move of the baseline names that the head's baseline file holds and the
/// base's does not: what the head may make worse, or loosen, on purpose.
fn new_moves(base: &Built, head: &Built) -> Vec<Vec<String>> {
    let Some(Ok(head)) = &head.baseline else {
        return Vec::new();
    };
    let known: &[baseline::Move] = match &base.baseline {
        Some(Ok(base)) => &base.moves,
        _ => &[],
    };
    head.moves
        .iter()
        .filter(|m| !known.contains(m))
        .map(|m| m.figures.clone())
        .collect()
}

/// The old baseline to check the head against, when this run's baseline is a move that has not
/// been checked: the pinned commit's own file names another commit, a move after the first
/// baseline is new to that file, and the push before, when given, did not hold that move yet.
/// Each later push leaves it, already checked at the push that made it.
fn move_to_check(
    pinned: &baseline::Baseline,
    base: &Built,
    options: &Options,
    repository: &Path,
) -> Option<String> {
    let Some(Ok(own)) = &base.baseline else {
        return None;
    };
    if own.commit == pinned.commit {
        return None;
    }
    let newest = pinned.moves.last()?;
    let is_first = pinned.moves.len() == 1;
    if is_first || own.moves.contains(newest) {
        return None;
    }
    if let Some(before) = &options.before {
        let earlier = build::git(
            repository,
            &["show", &format!("{before}:{}", baseline::PATH)],
        )
        .ok()
        .and_then(|text| baseline::parse(&text).ok());
        if earlier.is_some_and(|earlier| earlier.moves.contains(newest)) {
            return None;
        }
    }
    Some(own.commit.clone())
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

    fn with_baseline(moves: Vec<baseline::Move>) -> Built {
        Built {
            info: build::SideInfo::default(),
            programs: BTreeMap::new(),
            cases: BTreeMap::new(),
            baseline: Some(Ok(baseline::Baseline {
                commit: moves.last().expect("a move").commit.clone(),
                moves,
            })),
            thresholds: None,
            run_environment: Vec::new(),
        }
    }

    #[test]
    fn a_move_of_the_baseline_is_checked_against_the_old_one() {
        let options = read(&[]).unwrap();
        let repository = Path::new("/nowhere");
        let first = moved("a", &[]);
        let to_c = moved("c", &["partial: Pss_Anon"]);
        // The head pins c; c's own file still pins a, and lacks the move to c: checked against a.
        let pinned = baseline::Baseline {
            commit: to_c.commit.clone(),
            moves: vec![first.clone(), to_c.clone()],
        };
        let base = with_baseline(vec![first.clone()]);
        assert_eq!(
            move_to_check(&pinned, &base, &options, repository),
            Some("a".repeat(40))
        );
        // The figures a new move names, which the head may make worse or loosen.
        let head = with_baseline(vec![first.clone(), to_c.clone()]);
        assert_eq!(
            new_moves(&base, &head),
            [vec!["partial: Pss_Anon".to_owned()]]
        );
        assert!(new_moves(&head, &head).is_empty());
        // The base's own file pins itself, or already holds the move: nothing to check.
        let itself = with_baseline(vec![first.clone(), to_c.clone()]);
        assert_eq!(move_to_check(&pinned, &itself, &options, repository), None);
        // The first baseline replacing a placeholder has no old one to check against.
        let placeholder = with_baseline(vec![moved("e", &[])]);
        let first_pin = baseline::Baseline {
            commit: first.commit.clone(),
            moves: vec![first.clone()],
        };
        assert_eq!(
            move_to_check(&first_pin, &placeholder, &options, repository),
            None
        );
        // A base without the file, a revision from before it: nothing to check.
        let mut bare = with_baseline(vec![first]);
        bare.baseline = None;
        assert_eq!(move_to_check(&pinned, &bare, &options, repository), None);
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
            baseline: None,
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
