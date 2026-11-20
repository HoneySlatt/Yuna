//! The Vulkan prototype (ADR-0073): headless, on one GPU, it answers what Stage 2's renderer
//! needs to know of a machine. Which client buffers Vulkan imports, how it synchronises with
//! them, how a `wl_shm` buffer reaches the GPU, what a composition costs at the least, and whether
//! a high-priority queue is granted. It prints one report in Markdown and writes it in JSON.
//!
//! It opens no display and no surface: only the render node, through Vulkan. A throwaway, kept
//! on its own branch until Stage 2's renderer lands.

/// Writes one line to standard error, after the program's name. A closed or broken standard
/// error is ignored: progress and messages are not the report, which goes to standard output
/// and its JSON file.
macro_rules! progress {
    ($($argument:tt)*) => {{
        use std::io::Write as _;
        let _ = writeln!(std::io::stderr(), "vulkan-prototype: {}", format_args!($($argument)*));
    }};
}

mod compose;
mod dmabuf;
mod driver;
mod gpu;
mod json;
mod load;
mod modifiers;
mod outcome;
mod pattern;
mod priority;
mod scene;
mod scene_definition;
mod scene_report;
mod shm;
mod stats;
mod sync;
mod sys;
mod threads;

use std::env;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use gpu::{Gpu, ValidationMessage, Vulkan};
use json::Json;
use load::{GpuFiles, LoadMonitor, LoadWindow, Machine};

const USAGE: &str = "\
Usage:
  vulkan-prototype devices
      Lists the Vulkan devices, with their index.
  vulkan-prototype report [--device N] [--size WxH] [--json FILE] [--validate]
      Runs every part on device N (the first by default), with an output of WxH
      (3840x2160 by default). Prints the report in Markdown and writes it in JSON
      to FILE (vulkan-prototype.json by default). --validate turns on Khronos'
      validation layer and exits with 3 if it reported anything.
  vulkan-prototype scene [--device N] [--damage full|partial] [--warm-up N]
                         [--frames N] [--rate HZ] [--json FILE] [--capture FILE]
      Draws the scene the processor tier is measured on, as Yuna draws it, full
      or partial damage (full by default), 300 frames of warm-up then 1200
      measured, each at a 240 Hz boundary (--rate 0: back to back). Prints the
      figures in Markdown and writes them in JSON to FILE (scene.json by
      default); --capture writes the output after the last frame as a PAM image.
";

/// The version of the JSON file, raised when its fields change meaning.
// 2: the prototype's own GPU share is the graphics engine's alone (`own_gfx_percent`); each
// load window states the CPU estimate's bound and its GPU readings; `wl_shm`'s paths name their
// source memory.
const REPORT_FORMAT: i64 = 2;

/// How long the prototype idles before the timed parts, to read others' load alone.
const BASELINE: Duration = Duration::from_secs(2);

struct Options {
    device: usize,
    size: (u32, u32),
    json: PathBuf,
    validate: bool,
}

enum Command {
    Help,
    Devices,
    Report(Options),
    Scene(scene::SceneOptions),
}

fn main() -> ExitCode {
    let arguments: Vec<String> = env::args().skip(1).collect();
    match parse(&arguments) {
        Ok(Command::Help) => match print_out(USAGE) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                progress!("cannot print the usage: {error}");
                ExitCode::FAILURE
            }
        },
        Ok(Command::Devices) => devices(),
        Ok(Command::Report(options)) => report(&options, &arguments),
        Ok(Command::Scene(options)) => draw_scene(&options, &arguments),
        Err(message) => {
            progress!("{message}\n\n{USAGE}");
            ExitCode::from(2)
        }
    }
}

fn parse(arguments: &[String]) -> Result<Command, String> {
    let mut words = arguments.iter();
    match words.next().map(String::as_str) {
        None | Some("--help" | "-h" | "help") => return Ok(Command::Help),
        Some("devices") => {
            return match words.next() {
                None => Ok(Command::Devices),
                Some(word) => Err(format!("`devices` takes no argument, not `{word}`")),
            };
        }
        Some("report") => {}
        Some("scene") => return parse_scene(words).map(Command::Scene),
        Some(word) => return Err(format!("unknown command `{word}`")),
    }
    let mut options = Options {
        device: 0,
        size: (3840, 2160),
        json: PathBuf::from("vulkan-prototype.json"),
        validate: false,
    };
    while let Some(word) = words.next() {
        let mut value = || {
            words
                .next()
                .ok_or_else(|| format!("`{word}` needs a value"))
        };
        match word.as_str() {
            "--device" => {
                let value = value()?;
                options.device = value
                    .parse()
                    .map_err(|_| format!("`--device` takes an index, not `{value}`"))?;
            }
            "--size" => options.size = parse_size(value()?)?,
            "--json" => options.json = PathBuf::from(value()?),
            "--validate" => options.validate = true,
            other => return Err(format!("unknown option `{other}`")),
        }
    }
    Ok(Command::Report(options))
}

fn parse_scene<'a>(
    mut words: impl Iterator<Item = &'a String>,
) -> Result<scene::SceneOptions, String> {
    let mut options = scene::SceneOptions::new();
    while let Some(word) = words.next() {
        let value = words
            .next()
            .ok_or_else(|| format!("`{word}` needs a value"))?;
        let number = |least: usize| {
            value
                .parse::<usize>()
                .ok()
                .filter(|&number| number >= least)
                .ok_or_else(|| format!("`{word}` takes a whole number from {least}, not `{value}`"))
        };
        match word.as_str() {
            "--device" => options.device = number(0)?,
            "--damage" => options.damage = scene_report::Damage::parse(value)?,
            "--warm-up" => options.warm_up = number(0)?,
            "--frames" => options.frames = number(1)?,
            "--rate" => {
                options.rate_hz = value
                    .parse::<f64>()
                    .ok()
                    .filter(|rate| (0.0..=1000.0).contains(rate))
                    .ok_or_else(|| format!("`--rate` takes 0 to 1000 Hz, not `{value}`"))?
            }
            "--json" => options.json = PathBuf::from(value),
            "--capture" => options.capture = Some(PathBuf::from(value)),
            other => return Err(format!("unknown option `{other}`")),
        }
    }
    Ok(options)
}

/// Draws the scene, writes its JSON, then prints its Markdown.
fn draw_scene(options: &scene::SceneOptions, arguments: &[String]) -> ExitCode {
    let run = match scene::run(options, &arguments.join(" ")) {
        Ok(run) => run,
        Err(failure) => {
            progress!("{}", failure.describe());
            return ExitCode::FAILURE;
        }
    };
    if let Err(error) = write_atomically(&options.json, run.to_json().to_pretty_string().as_bytes())
    {
        progress!("cannot write {}: {error}", options.json.display());
        return ExitCode::FAILURE;
    }
    progress!("the JSON figures are in {}", options.json.display());
    match print_out(&run.to_markdown()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            progress!("cannot print the figures: {error}");
            ExitCode::FAILURE
        }
    }
}

fn parse_size(text: &str) -> Result<(u32, u32), String> {
    let error = || format!("`--size` takes WIDTHxHEIGHT, such as 3840x2160, not `{text}`");
    let (width, height) = text.split_once('x').ok_or_else(error)?;
    let width: u32 = width.parse().map_err(|_| error())?;
    let height: u32 = height.parse().map_err(|_| error())?;
    if !(1..=16384).contains(&width) || !(1..=16384).contains(&height) {
        return Err(format!("`--size` {text}: each side from 1 to 16384"));
    }
    Ok((width, height))
}

fn devices() -> ExitCode {
    let vulkan = match Vulkan::new(false) {
        Ok(vulkan) => vulkan,
        Err(failure) => {
            progress!("{}", failure.describe());
            return ExitCode::FAILURE;
        }
    };
    let physicals = match vulkan.physical_devices() {
        Ok(physicals) => physicals,
        Err(failure) => {
            progress!("{}", failure.describe());
            return ExitCode::FAILURE;
        }
    };
    let mut list = String::new();
    if physicals.is_empty() {
        list.push_str("No Vulkan device: the loader found no driver.\n");
    }
    for (index, &physical) in physicals.iter().enumerate() {
        let info = driver::read(&vulkan, physical);
        list.push_str(&format!(
            "{index}: {} ({}), Vulkan {}, {} {}\n",
            info.device_name,
            info.device_type,
            gpu::version_text(info.api_version),
            info.driver_name.as_deref().unwrap_or("unknown driver"),
            info.driver_info.as_deref().unwrap_or(""),
        ));
    }
    match print_out(&list) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            progress!("cannot print the devices: {error}");
            ExitCode::FAILURE
        }
    }
}

/// Writes `text` to standard output. A broken one, a pipe whose reader left, is an error the
/// caller reports, where `print!` would panic. A descriptor 1 closed at start the standard
/// library has already reopened on /dev/null, as it does 0 and 2, so output goes nowhere then.
fn print_out(text: &str) -> std::io::Result<()> {
    let mut out = std::io::stdout().lock();
    out.write_all(text.as_bytes())?;
    out.flush()
}

/// A part that ran, or why it did not.
enum Part<T> {
    Done(T),
    Skipped(String),
}

impl<T> Part<T> {
    fn to_json(&self, convert: impl FnOnce(&T) -> Json) -> Json {
        match self {
            Self::Done(value) => convert(value),
            Self::Skipped(reason) => Json::Object(vec![("skipped", Json::text(reason))]),
        }
    }

    fn to_markdown(&self, out: &mut String, render: impl FnOnce(&T, &mut String)) {
        match self {
            Self::Done(value) => render(value, out),
            Self::Skipped(reason) => out.push_str(&format!("Skipped: {reason}.\n\n")),
        }
    }
}

struct Parts {
    modifiers: Part<Vec<modifiers::FormatResult>>,
    sync: Part<sync::SyncResult>,
    shm: Part<Vec<shm::ShmResult>>,
    composition: Part<compose::ComposeResult>,
}

/// Parts 2 to 5, on two devices: a client's and a compositor's.
fn run_on_two_devices(
    vulkan: &Vulkan,
    physical: ash::vk::PhysicalDevice,
    options: &Options,
    load: &LoadMonitor,
    load_windows: &mut Vec<LoadWindow>,
) -> Parts {
    let (client, compositor) = match (Gpu::new(vulkan, physical), Gpu::new(vulkan, physical)) {
        (Ok(client), Ok(compositor)) => (client, compositor),
        (Err(failure), _) | (_, Err(failure)) => {
            let reason = format!("no device: {}", failure.describe());
            return Parts {
                modifiers: Part::Skipped(reason.clone()),
                sync: Part::Skipped(reason.clone()),
                shm: Part::Skipped(reason.clone()),
                composition: Part::Skipped(reason),
            };
        }
    };

    progress!("part 2, formats and modifiers");
    let modifiers = if client.modifiers.is_none() || !client.dma_buf || client.memory_fd.is_none() {
        Part::Skipped(
            "the device does not offer VK_EXT_image_drm_format_modifier, VK_EXT_external_memory_dma_buf \
             and VK_KHR_external_memory_fd together"
                .to_owned(),
        )
    } else {
        match modifiers::run(&client, &compositor) {
            Ok(results) => Part::Done(results),
            Err(failure) => Part::Skipped(failure.describe()),
        }
    };

    progress!("part 3, explicit sync");
    let sync = Part::Done(sync::run(&client, &compositor));

    progress!(
        "reading others' load for {} s before the timed parts",
        BASELINE.as_secs()
    );
    let start = load.mark();
    std::thread::sleep(BASELINE);
    load_windows.push(load.window("before the timed parts, the prototype idle", start));

    progress!("part 4, wl_shm's path");
    let shm = Part::Done(shm::run(&compositor, load, load_windows));

    progress!("part 5, the composition's floor");
    let composition = match &modifiers {
        Part::Done(results) => {
            let working = modifiers::working_modifiers(results, ash::vk::Format::B8G8R8A8_UNORM);
            match compose::run(
                &client,
                &compositor,
                &working,
                options.size,
                load,
                load_windows,
            ) {
                Ok(result) => Part::Done(result),
                Err(failure) => Part::Skipped(failure.describe()),
            }
        }
        Part::Skipped(reason) => Part::Skipped(format!(
            "its windows are dma-bufs, and part 2 did not run: {reason}"
        )),
    };
    Parts {
        modifiers,
        sync,
        shm,
        composition,
    }
}

fn report(options: &Options, arguments: &[String]) -> ExitCode {
    let date = utc_date(SystemTime::now());
    let vulkan = match Vulkan::new(options.validate) {
        Ok(vulkan) => vulkan,
        Err(failure) => {
            progress!("{}", failure.describe());
            return ExitCode::FAILURE;
        }
    };
    let physicals = match vulkan.physical_devices() {
        Ok(physicals) => physicals,
        Err(failure) => {
            progress!("{}", failure.describe());
            return ExitCode::FAILURE;
        }
    };
    let Some(&physical) = physicals.get(options.device) else {
        progress!(
            "there is no device {} ({} found); `vulkan-prototype devices` lists them",
            options.device,
            physicals.len()
        );
        return ExitCode::FAILURE;
    };

    let machine = Machine::read();
    progress!("part 1, the driver");
    let driver = driver::read(&vulkan, physical);
    let load = LoadMonitor::start(
        driver
            .render_node
            .and_then(|node| GpuFiles::of_render_node(node.major, node.minor)),
        Vec::new(),
    );
    let mut load_windows = Vec::new();
    let parts = run_on_two_devices(&vulkan, physical, options, &load, &mut load_windows);
    progress!("part 6, priority");
    let priority = priority::run(&vulkan, physical);
    drop(load);
    let validation = vulkan.validation_messages();

    let header = Header {
        date,
        arguments,
        options,
        validating: vulkan.validating(),
    };
    // The JSON file first: it is kept whatever becomes of standard output.
    let json = report_json(
        &header,
        &machine,
        &driver,
        &parts,
        &priority,
        &load_windows,
        &validation,
    );
    match write_atomically(&options.json, json.to_pretty_string().as_bytes()) {
        Ok(()) => progress!("the JSON report is in {}", options.json.display()),
        Err(error) => {
            progress!("cannot write {}: {error}", options.json.display());
            return ExitCode::FAILURE;
        }
    }
    let text = markdown(
        &header,
        &machine,
        &driver,
        &parts,
        &priority,
        &load_windows,
        &validation,
    );
    if let Err(error) = print_out(&text) {
        progress!(
            "cannot print the Markdown report ({error}); the JSON report is in {}",
            options.json.display()
        );
        return ExitCode::FAILURE;
    }
    let findings = validation
        .iter()
        .filter(|message| is_finding(message))
        .count();
    if options.validate && findings > 0 {
        progress!("the validation layer reported {findings} kinds of message");
        return ExitCode::from(3);
    }
    ExitCode::SUCCESS
}

/// A message about the prototype's use of Vulkan: any error, and any warning of the layer's
/// checks. The loader's warnings about the machine's other drivers are not.
fn is_finding(message: &ValidationMessage) -> bool {
    message.severity == "error" || message.kind != "general"
}

struct Header<'a> {
    date: String,
    arguments: &'a [String],
    options: &'a Options,
    validating: bool,
}

/// The environment variables that choose Vulkan's drivers and layers, when set.
fn vulkan_environment() -> Vec<(&'static str, String)> {
    [
        "VK_ICD_FILENAMES",
        "VK_DRIVER_FILES",
        "VK_ADD_DRIVER_FILES",
        "VK_INSTANCE_LAYERS",
    ]
    .into_iter()
    .filter_map(|name| env::var(name).ok().map(|value| (name, value)))
    .collect()
}

fn markdown(
    header: &Header<'_>,
    machine: &Machine,
    driver: &driver::DriverInfo,
    parts: &Parts,
    priority: &priority::PriorityResult,
    load_windows: &[LoadWindow],
    validation: &[ValidationMessage],
) -> String {
    let mut out = String::new();
    out.push_str("# Vulkan prototype report\n\n");
    out.push_str(&format!("- Date: {}\n", header.date));
    out.push_str(&format!(
        "- Prototype commit: `{}`\n",
        env!("PROTOTYPE_COMMIT")
    ));
    out.push_str(&format!(
        "- Command: `vulkan-prototype {}`\n",
        header.arguments.join(" ")
    ));
    out.push_str(&format!(
        "- Device {}, output {}x{}, validation {}\n",
        header.options.device,
        header.options.size.0,
        header.options.size.1,
        if header.validating { "on" } else { "off" }
    ));
    for (name, value) in vulkan_environment() {
        out.push_str(&format!("- `{name}={value}`\n"));
    }
    let disturbed: Vec<&str> = load_windows
        .iter()
        .filter(|window| window.disturbed)
        .map(|window| window.name.as_str())
        .collect();
    if disturbed.is_empty() {
        out.push_str("- Timed parts: none disturbed by other programs (part 7)\n");
    } else {
        out.push_str(&format!(
            "- **Timed parts disturbed by other programs** (part 7): {}\n",
            disturbed.join("; ")
        ));
    }
    out.push_str("\n## The machine\n\n");
    machine.to_markdown(&mut out);
    out.push_str("## 1. The driver\n\n");
    driver.to_markdown(&mut out);
    out.push_str("## 2. Formats and modifiers\n\n");
    parts.modifiers.to_markdown(&mut out, |results, out| {
        modifiers::to_markdown(results, out)
    });
    out.push_str("## 3. Explicit sync\n\n");
    parts.sync.to_markdown(&mut out, sync::to_markdown);
    out.push_str("## 4. `wl_shm`'s path\n\n");
    parts
        .shm
        .to_markdown(&mut out, |results, out| shm::to_markdown(results, out));
    out.push_str("## 5. The composition's floor\n\n");
    parts
        .composition
        .to_markdown(&mut out, compose::to_markdown);
    out.push_str("## 6. Priority\n\n");
    priority::to_markdown(priority, &mut out);
    out.push_str("## 7. The machine's load\n\n");
    load::to_markdown(load_windows, &mut out);
    out.push_str("## Validation\n\n");
    if !header.validating {
        out.push_str("Off.\n");
    } else {
        let findings = validation
            .iter()
            .filter(|message| is_finding(message))
            .count();
        if findings == 0 {
            out.push_str("On; Khronos' layer reported no error and no warning of its checks.\n");
        } else {
            out.push_str(&format!(
                "On; Khronos' layer reported {findings} kinds of message of its checks.\n"
            ));
        }
        if !validation.is_empty() {
            out.push_str(
                "\nEvery message, the loader's own about the machine's drivers included:\n\n\
                 | Severity | Kind | Id | Count | First message |\n|---|---|---|---|---|\n",
            );
        }
        for message in validation {
            let text: String = message.first_text.chars().take(400).collect();
            out.push_str(&format!(
                "| {} | {} | `{}` | {} | {} |\n",
                message.severity,
                message.kind,
                message.id,
                message.count,
                text.replace('\n', " ").replace('|', "\\|")
            ));
        }
    }
    out
}

fn report_json(
    header: &Header<'_>,
    machine: &Machine,
    driver: &driver::DriverInfo,
    parts: &Parts,
    priority: &priority::PriorityResult,
    load_windows: &[LoadWindow],
    validation: &[ValidationMessage],
) -> Json {
    Json::Object(vec![
        ("format", Json::Integer(REPORT_FORMAT)),
        ("date", Json::text(&header.date)),
        ("commit", Json::text(env!("PROTOTYPE_COMMIT"))),
        (
            "arguments",
            Json::Array(header.arguments.iter().map(Json::text).collect()),
        ),
        ("device_index", Json::Integer(header.options.device as i64)),
        (
            "output_size",
            Json::text(format!(
                "{}x{}",
                header.options.size.0, header.options.size.1
            )),
        ),
        (
            "environment",
            Json::Object(
                vulkan_environment()
                    .into_iter()
                    .map(|(name, value)| (name, Json::Text(value)))
                    .collect(),
            ),
        ),
        ("machine", machine.to_json()),
        ("driver", driver.to_json()),
        (
            "modifiers",
            parts
                .modifiers
                .to_json(|results| modifiers::to_json(results)),
        ),
        ("sync", parts.sync.to_json(sync::to_json)),
        ("shm", parts.shm.to_json(|results| shm::to_json(results))),
        ("composition", parts.composition.to_json(compose::to_json)),
        ("priority", priority::to_json(priority)),
        (
            "load",
            Json::Array(load_windows.iter().map(LoadWindow::to_json).collect()),
        ),
        (
            "validation",
            Json::Object(vec![
                ("enabled", Json::Bool(header.validating)),
                (
                    "messages",
                    Json::Array(
                        validation
                            .iter()
                            .map(|message| {
                                Json::Object(vec![
                                    ("severity", Json::text(message.severity)),
                                    ("kind", Json::text(message.kind)),
                                    ("finding", Json::Bool(is_finding(message))),
                                    ("id", Json::text(&message.id)),
                                    ("count", Json::Integer(i64::from(message.count))),
                                    ("first_text", Json::text(&message.first_text)),
                                ])
                            })
                            .collect(),
                    ),
                ),
            ]),
        ),
    ])
}

/// Writes `bytes` to `path` through a temporary file in the same folder and a rename, so that a
/// reader never sees half a report and an interrupted run leaves the last one whole.
fn write_atomically(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let name = path
        .file_name()
        .ok_or_else(|| std::io::Error::other("the path names no file"))?;
    let mut temporary_name = std::ffi::OsString::from(".");
    temporary_name.push(name);
    temporary_name.push(format!(".{}.tmp", std::process::id()));
    let temporary = path.with_file_name(temporary_name);
    let result = (|| {
        let mut file = fs::File::create(&temporary)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        fs::rename(&temporary, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

/// `time` in UTC, as ISO 8601: 2026-11-15T18:52:00Z.
fn utc_date(time: SystemTime) -> String {
    let seconds = time
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs());
    let (days, rest) = ((seconds / 86_400) as i64, seconds % 86_400);
    // Days since 1970-01-01 to a civil date, after Howard Hinnant's `civil_from_days`.
    let shifted = days + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rest / 3600,
        rest / 60 % 60,
        rest % 60
    )
}

#[cfg(test)]
mod tests {
    use super::{Command, parse, parse_size, utc_date};
    use std::time::{Duration, UNIX_EPOCH};

    #[test]
    fn writes_dates_in_utc() {
        assert_eq!(utc_date(UNIX_EPOCH), "1970-01-01T00:00:00Z");
        assert_eq!(
            utc_date(UNIX_EPOCH + Duration::from_secs(951_782_400 + 3_723)),
            "2000-02-29T01:02:03Z"
        );
    }

    #[test]
    fn reads_the_arguments() {
        let words = |text: &str| {
            text.split_whitespace()
                .map(str::to_owned)
                .collect::<Vec<_>>()
        };
        let Ok(Command::Report(options)) =
            parse(&words("report --device 1 --size 1920x1080 --validate"))
        else {
            panic!("a valid report command");
        };
        assert_eq!(
            (options.device, options.size, options.validate),
            (1, (1920, 1080), true)
        );
        assert!(parse(&words("report --size 0x10")).is_err());
        assert!(parse(&words("report --device")).is_err());
        assert!(parse(&words("report --colour")).is_err());
        assert!(matches!(parse(&words("devices")), Ok(Command::Devices)));
        let Ok(Command::Scene(scene)) =
            parse(&words("scene --damage partial --frames 1000 --rate 0"))
        else {
            panic!("a valid scene command");
        };
        assert_eq!(
            (scene.damage, scene.frames, scene.rate_hz),
            (crate::scene_report::Damage::Partial, 1000, 0.0)
        );
        assert!(parse(&words("scene --damage some")).is_err());
        assert!(parse(&words("scene --frames 0")).is_err());
        assert!(parse_size("3840x").is_err());
    }
}
