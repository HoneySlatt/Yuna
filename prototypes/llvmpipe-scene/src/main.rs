//! `llvmpipe-scene`: the scene the processor tier is judged on (ADR-0139), drawn by today's Yuna
//! on Mesa's llvmpipe and measured as the Vulkan prototype's `scene` measures its own drawing on
//! lavapipe. Throwaway, on the branch `prototype/vulkan`, which is never merged.
//!
//! It starts `yuna --headless` (`yuna.rs`), opens the scene's two windows as a `wl_shm` client
//! (`client.rs`), and commits a frame at each frame callback, full or partial damage. Over the
//! measured frames it reads each frame's time from Yuna's log, the CPU time of Yuna's threads,
//! and other programs' load, with the prototype's own code, which it includes: the scene's
//! definition, the load's sampler, the threads' times, the summary and the report. It then
//! captures the output and checks it against the scene's definition.

#[path = "../../vulkan/src/json.rs"]
mod json;
#[path = "../../vulkan/src/load.rs"]
mod load;
#[path = "../../vulkan/src/scene_definition.rs"]
mod scene_definition;
#[path = "../../vulkan/src/scene_report.rs"]
mod scene_report;
#[path = "../../vulkan/src/stats.rs"]
mod stats;
#[path = "../../vulkan/src/threads.rs"]
mod threads;

mod client;
mod yuna;

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, Instant};

use client::Client;
use load::{GpuFiles, LoadMonitor, Machine};
use scene_definition::{check_image, wait_for_boundary, write_pam};
use scene_report::{Damage, SceneRun, rasterizer_threads, step_counts};
use threads::{Threads, Usage};
use yuna::Yuna;

const USAGE: &str = "\
Usage:
  llvmpipe-scene --yuna PATH [--damage full|partial] [--warm-up N] [--frames N]
                 [--rate HZ] [--json FILE] [--log FILE] [--capture FILE]
                 [--yuna-log LEVEL]
      Starts `PATH --headless` (Yuna, on llvmpipe, at its default output), draws
      the scene in it as a wl_shm client, full or partial damage (full by default),
      300 frames of warm-up then 1200 measured, and reads each frame's time from
      Yuna's log, which goes to FILE (llvmpipe-scene-yuna.log by default), at LEVEL
      (debug by default). Prints the figures in Markdown and writes them in JSON to
      FILE (llvmpipe-scene.json by default); --capture writes the output after the
      last frame as a PAM image. It commits each frame at Yuna's frame callback for
      the one before, or with --rate at the first boundary of HZ after that
      callback, as the prototype's `scene --rate` paces its own. Run it inside
      `nix develop`, which gives Yuna its
      Mesa, and inside `agent-headless`. LP_NUM_THREADS, when set, is given to Yuna.
";

struct Options {
    yuna: PathBuf,
    damage: Damage,
    warm_up: u64,
    frames: u64,
    json: PathBuf,
    log: PathBuf,
    log_level: String,
    capture: Option<PathBuf>,
    /// Commits a second at most, at its boundaries; 0 commits at each frame callback.
    rate_hz: f64,
}

fn main() -> ExitCode {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let options = match parse(&arguments) {
        Ok(Some(options)) => options,
        Ok(None) => return print_out(USAGE),
        Err(message) => {
            eprintln!("llvmpipe-scene: {message}\n\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    let run = match measure(&options, &arguments.join(" ")) {
        Ok(run) => run,
        Err(message) => {
            eprintln!("llvmpipe-scene: {message}");
            return ExitCode::FAILURE;
        }
    };
    if let Err(error) = write_atomically(&options.json, &run.to_json().to_pretty_string()) {
        eprintln!(
            "llvmpipe-scene: cannot write {}: {error}",
            options.json.display()
        );
        return ExitCode::FAILURE;
    }
    eprintln!(
        "llvmpipe-scene: the JSON figures are in {}",
        options.json.display()
    );
    print_out(&run.to_markdown())
}

/// The options, or none when help is asked.
fn parse(arguments: &[String]) -> Result<Option<Options>, String> {
    let mut options = Options {
        yuna: PathBuf::new(),
        damage: Damage::Full,
        warm_up: 300,
        frames: 1200,
        json: PathBuf::from("llvmpipe-scene.json"),
        log: PathBuf::from("llvmpipe-scene-yuna.log"),
        log_level: "debug".to_owned(),
        capture: None,
        rate_hz: 0.0,
    };
    let mut words = arguments.iter();
    while let Some(word) = words.next() {
        if matches!(word.as_str(), "--help" | "-h") {
            return Ok(None);
        }
        let value = words
            .next()
            .ok_or_else(|| format!("`{word}` needs a value"))?;
        let number = |least: u64| {
            value
                .parse::<u64>()
                .ok()
                .filter(|&number| number >= least)
                .ok_or_else(|| format!("`{word}` takes a whole number from {least}, not `{value}`"))
        };
        match word.as_str() {
            "--yuna" => options.yuna = PathBuf::from(value),
            "--damage" => options.damage = Damage::parse(value)?,
            "--warm-up" => options.warm_up = number(0)?,
            "--frames" => options.frames = number(1)?,
            "--json" => options.json = PathBuf::from(value),
            "--log" => options.log = PathBuf::from(value),
            "--yuna-log" => options.log_level = value.clone(),
            "--capture" => options.capture = Some(PathBuf::from(value)),
            "--rate" => {
                options.rate_hz = value
                    .parse::<f64>()
                    .ok()
                    .filter(|rate| (0.0..=1000.0).contains(rate))
                    .ok_or_else(|| format!("`--rate` takes 0 to 1000 Hz, not `{value}`"))?
            }
            other => return Err(format!("unknown option `{other}`")),
        }
    }
    if options.yuna.as_os_str().is_empty() {
        return Err("`--yuna PATH` names the yuna program to measure".to_owned());
    }
    Ok(Some(options))
}

/// Runs Yuna and the scene's client in a folder of their own, removed at the end.
fn measure(options: &Options, command: &str) -> Result<SceneRun, String> {
    let parent = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let folder = parent.join(format!("llvmpipe-scene-{}", std::process::id()));
    let result = measure_in(&folder, options, command);
    let _ = std::fs::remove_dir_all(&folder);
    result
}

fn measure_in(folder: &Path, options: &Options, command: &str) -> Result<SceneRun, String> {
    let mut yuna = Yuna::start(&options.yuna, folder, &options.log, &options.log_level)?;
    let pid = yuna.pid();
    let machine = Machine::read();
    // Yuna's processor time is the measure's own, not others' load.
    let load = LoadMonitor::start(GpuFiles::first_render_node(), vec![pid]);
    let mut client = Client::connect(&yuna.socket)?;
    client.open_windows()?;
    eprintln!(
        "llvmpipe-scene: {} damage, {} frames of warm-up then {} measured",
        options.damage.name(),
        options.warm_up,
        options.frames
    );
    let total = options.warm_up + options.frames;
    let period = (options.rate_hz > 0.0).then(|| Duration::from_secs_f64(1.0 / options.rate_hz));
    let origin = Instant::now();
    let mut called_back = origin;
    let mut window_start = None;
    for number in 1..=total {
        if number == options.warm_up + 1 {
            window_start = Some((yuna.log_length()?, Threads::read(pid), load.mark()));
        }
        if let Some(period) = period {
            wait_for_boundary(origin, called_back, period);
        }
        client.frame(number, options.damage)?;
        called_back = Instant::now();
        if !yuna.running() {
            return Err("Yuna stopped during the measure".to_owned());
        }
    }
    let (start, threads_before, load_start) = window_start.ok_or("no frame was measured")?;
    let end = yuna.log_length()?;
    let cpu = Usage::between(&threads_before, &Threads::read(pid));
    let load_window = load.window("the measured frames", load_start);
    drop(load);
    let log = yuna.read_log()?;
    let drawn = yuna::frames(&log[start..end]);
    let steps = step_counts(
        drawn
            .boundaries
            .windows(2)
            .map(|pair| pair[1].saturating_sub(pair[0])),
    );

    let pixels = client.capture()?;
    // Full damage never changes the region; partial damage leaves the last frame's in it.
    let content_frame = match options.damage {
        Damage::Full => 0,
        Damage::Partial => total,
    };
    let image = check_image(&pixels, content_frame).describe();
    if let Some(path) = &options.capture {
        write_pam(path, &pixels)
            .map_err(|error| format!("cannot write {}: {error}", path.display()))?;
        eprintln!("llvmpipe-scene: the output is in {}", path.display());
    }
    let warnings: Vec<&str> = log
        .lines()
        .filter(|line| line.contains(" WARN ") || line.contains(" ERROR "))
        .take(5)
        .collect();
    let range = |(least, most): (u64, u64)| {
        if least > most {
            "none".to_owned()
        } else {
            format!("{least} to {most}")
        }
    };
    let facts = vec![
        ("Command", format!("`llvmpipe-scene {command}`")),
        ("Yuna", format!("`{} --headless`", options.yuna.display())),
        ("Yuna's log", format!("`{}`", options.log.display())),
        (
            "Frames",
            format!(
                "{} committed after {} of warm-up and a first frame of the windows' opening; \
                 {} `frame drawn` lines in the measured window, each damaging {} rectangles and \
                 drawing {} elements",
                options.frames,
                options.warm_up,
                drawn.took_us.len(),
                range(drawn.damaged_rectangles),
                range(drawn.elements)
            ),
        ),
        (
            "Wall time",
            "Yuna's `took_us`: from the start of its frame, its clients' buffers uploaded, to \
             its drawing done and its frame callbacks sent, rounded down to whole microseconds"
                .to_owned(),
        ),
        (
            "Pacing",
            match period {
                Some(_) => format!(
                    "each frame committed at the first {} Hz boundary after Yuna's frame \
                     callback for the one before, and drawn at Yuna's next refresh",
                    options.rate_hz
                ),
                None => "each frame committed at Yuna's frame callback for the one before; \
                         Yuna's frame loop sets the cadence"
                    .to_owned(),
            },
        ),
        (
            "Client",
            format!(
                "one process, `wl_shm` `XRGB8888` buffers at the windows' physical size, its \
                 viewport at their logical size; buffers made by window: {:?}",
                client.buffers_made()
            ),
        ),
        (
            "Yuna's warnings and errors",
            if warnings.is_empty() {
                "none".to_owned()
            } else {
                warnings.join(" / ")
            },
        ),
    ];
    Ok(SceneRun {
        side: "Yuna, llvmpipe",
        damage: options.damage,
        rasterizer_threads: rasterizer_threads(),
        wall_us: drawn.took_us,
        cpu,
        load: load_window,
        machine,
        image,
        pacing: match period {
            Some(_) => format!("{} Hz", options.rate_hz),
            None => "frame callbacks".to_owned(),
        },
        steps,
        facts,
    })
}

/// Writes `text` to `path` through a temporary file and a rename.
fn write_atomically(path: &Path, text: &str) -> std::io::Result<()> {
    let mut temporary = path.as_os_str().to_owned();
    temporary.push(".tmp");
    std::fs::write(&temporary, text)?;
    std::fs::rename(&temporary, path)
}

/// Writes `text` to standard output; a broken one is a failure, not a panic.
fn print_out(text: &str) -> ExitCode {
    let mut out = std::io::stdout().lock();
    match out.write_all(text.as_bytes()).and_then(|()| out.flush()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("llvmpipe-scene: cannot print the figures: {error}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::parse;
    use crate::scene_report::Damage;

    #[test]
    fn reads_the_arguments() {
        let words = |text: &str| {
            text.split_whitespace()
                .map(str::to_owned)
                .collect::<Vec<_>>()
        };
        let Ok(Some(options)) = parse(&words(
            "--yuna /bin/yuna --damage partial --frames 1000 --rate 80",
        )) else {
            panic!("valid arguments");
        };
        assert_eq!(
            (
                options.damage,
                options.frames,
                options.warm_up,
                options.rate_hz
            ),
            (Damage::Partial, 1000, 300, 80.0)
        );
        assert!(parse(&words("--yuna y --rate fast")).is_err());
        assert!(matches!(parse(&words("--help")), Ok(None)));
        assert!(parse(&words("--damage full")).is_err());
        assert!(parse(&words("--yuna y --frames 0")).is_err());
        assert!(parse(&words("--yuna y --colour red")).is_err());
    }
}
