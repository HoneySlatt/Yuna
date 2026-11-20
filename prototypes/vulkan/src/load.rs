//! Part 7: the load other programs put on the machine while the prototype times its parts, as
//! ADR-0060 asks of every timed measure, and the machine's own facts.
//!
//! A thread reads the counters each second; each timed part also reads them at its start and its
//! end. Both figures are estimates:
//!
//! - Other programs' CPU is the machine's busy time less the prototype's own, `/proc/stat` less
//!   `/proc/self/stat`, and less the processes it measures with it (`LoadMonitor::start`'s
//!   `measured`: Yuna, for `llvmpipe-scene`). All count in ticks of 10 ms (USER_HZ), each field
//!   rounded down: the six busy fields, and two fields of each own process, make the difference
//!   wrong by less than 8 ticks for the prototype alone, 0.08 s of CPU time over the part, and 2
//!   ticks more for each process measured with it, before the kernel's own sampling of what runs
//!   at each tick. Each window states that bound in logical CPUs: 0.4 over a 0.2 s part, 0.006
//!   over 13 s, for the prototype alone.
//! - Other programs' GPU is amdgpu's `gpu_busy_percent`, the graphics engine's activity, read
//!   after the part's start: at its end and once each second during it, so once or twice for a
//!   part shorter than a second. Less the prototype's own share of the graphics engine over the
//!   part, from the kernel's `drm-engine-gfx` in `/proc/self/fdinfo`.
//!
//! A part is disturbed when others used more than 2 logical CPUs or 10 % of the GPU on average.
//! `LoadWindow::disturbed_in_a_second` is stricter, for the scene: it also asks that no second
//! of the part went over either limit, beyond the counters' rounding over that second.
//! A part that measured nothing has no window.

use std::fs;
use std::path::PathBuf;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::json::Json;

const OTHERS_CORES_LIMIT: f64 = 2.0;
const OTHERS_GPU_PERCENT_LIMIT: f64 = 10.0;
/// The most the rounding of the CPU counters can shift others' busy time, in ticks: six fields
/// of `/proc/stat`'s busy time and two of each own process's `stat`, the prototype's and those
/// it measures, each rounded down to a tick.
fn cpu_rounding_ticks(own_processes: usize) -> f64 {
    (6 + 2 * own_processes) as f64
}

/// The files of the GPU the prototype runs on: its sysfs folder and its render node.
pub struct GpuFiles {
    device: PathBuf,
    node: PathBuf,
}

impl GpuFiles {
    /// The GPU whose render node is the character device `major:minor`, when sysfs has it.
    pub fn of_render_node(major: i64, minor: i64) -> Option<Self> {
        let node = fs::canonicalize(format!("/sys/dev/char/{major}:{minor}")).ok()?;
        let name = node.file_name()?.to_str()?.to_owned();
        Some(Self {
            device: node.join("device"),
            node: PathBuf::from("/dev/dri").join(name),
        })
    }

    /// The GPU of the machine's first render node, for a driver that has none, such as
    /// lavapipe: others' load on the machine's GPU is read all the same, the prototype's own
    /// being none. Read from sysfs; the node is never opened.
    pub fn first_render_node() -> Option<Self> {
        use std::os::unix::fs::MetadataExt;
        let mut nodes: Vec<PathBuf> = fs::read_dir("/dev/dri")
            .ok()?
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with("renderD"))
            })
            .collect();
        nodes.sort();
        let device = fs::metadata(nodes.first()?).ok()?.rdev();
        // Linux's dev_t: the major in bits 8-19 and 32-43, the minor in bits 0-7 and 20-31.
        let major = ((device >> 8) & 0xfff) | ((device >> 32) & 0xffff_f000);
        let minor = (device & 0xff) | ((device >> 12) & 0xffff_ff00);
        Self::of_render_node(major as i64, minor as i64)
    }
}

#[derive(Clone)]
struct Sample {
    at: Instant,
    /// Busy and total ticks of every CPU, and how many CPUs `/proc/stat` lists.
    cpu: Option<(u64, u64, u32)>,
    /// The CPU ticks of the prototype's own threads and of the processes it measures.
    own_ticks: Option<u64>,
    gpu_busy_percent: Option<f64>,
    /// The prototype's own GPU time on the device, in nanoseconds, every engine's.
    own_gpu_ns: Option<u64>,
    performance_level: Option<String>,
    sclk_mhz: Option<u32>,
    mclk_mhz: Option<u32>,
}

fn take_sample(gpu: Option<&GpuFiles>, measured: &[u32]) -> Sample {
    let read = |name: &str| gpu.and_then(|gpu| fs::read_to_string(gpu.device.join(name)).ok());
    Sample {
        at: Instant::now(),
        cpu: machine_cpu_ticks(),
        own_ticks: own_cpu_ticks(measured),
        gpu_busy_percent: read("gpu_busy_percent").and_then(|text| text.trim().parse().ok()),
        own_gpu_ns: gpu.map(own_gpu_ns),
        performance_level: read("power_dpm_force_performance_level")
            .map(|text| text.trim().to_owned()),
        sclk_mhz: read("pp_dpm_sclk").and_then(|text| current_clock(&text)),
        mclk_mhz: read("pp_dpm_mclk").and_then(|text| current_clock(&text)),
    }
}

/// `/proc/stat`'s first line: busy ticks (user, nice, system, irq, softirq, steal) and all
/// ticks (with idle and iowait); guest time is inside user time already.
fn machine_cpu_ticks() -> Option<(u64, u64, u32)> {
    let text = fs::read_to_string("/proc/stat").ok()?;
    parse_proc_stat(&text)
}

fn parse_proc_stat(text: &str) -> Option<(u64, u64, u32)> {
    let mut lines = text.lines();
    let first = lines.next()?;
    let fields: Vec<u64> = first
        .strip_prefix("cpu ")?
        .split_whitespace()
        .map(|field| field.parse().ok())
        .collect::<Option<_>>()?;
    let field = |index: usize| fields.get(index).copied().unwrap_or(0);
    let busy = field(0) + field(1) + field(2) + field(5) + field(6) + field(7);
    let total = busy + field(3) + field(4);
    let cpus = lines.take_while(|line| line.starts_with("cpu")).count() as u32;
    Some((busy, total, cpus.max(1)))
}

/// utime and stime from `/proc/self/stat` and each measured process's `stat`, fields 14 and 15,
/// after the name in parentheses. None if any cannot be read, such as a process that ended.
fn own_cpu_ticks(measured: &[u32]) -> Option<u64> {
    let mut ticks = parse_own_stat(&fs::read_to_string("/proc/self/stat").ok()?)?;
    for pid in measured {
        ticks += parse_own_stat(&fs::read_to_string(format!("/proc/{pid}/stat")).ok()?)?;
    }
    Some(ticks)
}

fn parse_own_stat(text: &str) -> Option<u64> {
    let after_name = &text[text.rfind(')')? + 1..];
    let fields: Vec<&str> = after_name.split_whitespace().collect();
    let utime: u64 = fields.get(11)?.parse().ok()?;
    let stime: u64 = fields.get(12)?.parse().ok()?;
    Some(utime + stime)
}

/// The graphics engine's time of the prototype's DRM clients on the GPU's render node, from the
/// kernel's `drm-engine-gfx` lines, each client counted once: the engine `gpu_busy_percent`
/// measures, and the one the prototype's universal queue runs on.
fn own_gpu_ns(gpu: &GpuFiles) -> u64 {
    let Ok(entries) = fs::read_dir("/proc/self/fd") else {
        return 0;
    };
    let mut clients = Vec::new();
    let mut total = 0;
    for entry in entries.flatten() {
        if fs::read_link(entry.path()).ok().as_deref() != Some(gpu.node.as_path()) {
            continue;
        }
        let Ok(info) =
            fs::read_to_string(PathBuf::from("/proc/self/fdinfo").join(entry.file_name()))
        else {
            continue;
        };
        let (client, graphics_ns) = parse_fdinfo(&info);
        if let Some(client) = client {
            if clients.contains(&client) {
                continue;
            }
            clients.push(client);
        }
        total += graphics_ns;
    }
    total
}

/// A DRM client's id and its graphics engine's time in nanoseconds.
fn parse_fdinfo(text: &str) -> (Option<u64>, u64) {
    let mut client = None;
    let mut graphics_ns = 0;
    for line in text.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim();
        if key == "drm-client-id" {
            client = value.parse().ok();
        } else if key == "drm-engine-gfx"
            && let Some(ns) = value
                .strip_suffix(" ns")
                .and_then(|ns| ns.trim().parse::<u64>().ok())
        {
            graphics_ns += ns;
        }
    }
    (client, graphics_ns)
}

/// The clock marked current in `pp_dpm_sclk` or `pp_dpm_mclk`: "1: 2530Mhz *".
fn current_clock(text: &str) -> Option<u32> {
    let line = text.lines().find(|line| line.trim_end().ends_with('*'))?;
    let (_, value) = line.split_once(':')?;
    let digits: String = value
        .trim()
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    digits.parse().ok()
}

/// The sampler: a thread reading the counters each second, and marks at the parts' bounds.
pub struct LoadMonitor {
    samples: Arc<Mutex<Vec<Sample>>>,
    gpu: Option<Arc<GpuFiles>>,
    /// The processes whose CPU counts as the prototype's own, not others'.
    measured: Arc<[u32]>,
    stop: Option<mpsc::Sender<()>>,
    thread: Option<JoinHandle<()>>,
}

impl LoadMonitor {
    /// Starts sampling the load of `gpu` and of the processor, where the processes `measured`
    /// count as the prototype's own.
    pub fn start(gpu: Option<GpuFiles>, measured: Vec<u32>) -> Self {
        let gpu = gpu.map(Arc::new);
        let measured: Arc<[u32]> = measured.into();
        let samples = Arc::new(Mutex::new(Vec::new()));
        let (stop, stopped) = mpsc::channel::<()>();
        let thread = {
            let samples = Arc::clone(&samples);
            let gpu = gpu.clone();
            let measured = Arc::clone(&measured);
            std::thread::Builder::new()
                .name("load".to_owned())
                .spawn(move || {
                    while let Err(RecvTimeoutError::Timeout) =
                        stopped.recv_timeout(Duration::from_secs(1))
                    {
                        push_sample(&samples, gpu.as_deref(), &measured);
                    }
                })
                .ok()
        };
        Self {
            samples,
            gpu,
            measured,
            stop: Some(stop),
            thread,
        }
    }

    /// Reads the counters now; returns the sample's place, the start of a part.
    pub fn mark(&self) -> usize {
        push_sample(&self.samples, self.gpu.as_deref(), &self.measured)
    }

    /// Reads the counters now and sums up the part that began at `start`.
    pub fn window(&self, name: impl Into<String>, start: usize) -> LoadWindow {
        let end = self.mark();
        let samples = self
            .samples
            .lock()
            .ok()
            .and_then(|samples| samples.get(start.min(end)..=end).map(<[Sample]>::to_vec))
            .unwrap_or_default();
        LoadWindow::of(
            name.into(),
            &samples,
            cpu_rounding_ticks(1 + self.measured.len()),
        )
    }
}

impl Drop for LoadMonitor {
    fn drop(&mut self) {
        drop(self.stop.take());
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Reads a sample and appends it under the lock, so that samples stay in time order.
fn push_sample(samples: &Mutex<Vec<Sample>>, gpu: Option<&GpuFiles>, measured: &[u32]) -> usize {
    match samples.lock() {
        Ok(mut samples) => {
            samples.push(take_sample(gpu, measured));
            samples.len() - 1
        }
        Err(_) => 0,
    }
}

/// The load during one timed part.
pub struct LoadWindow {
    pub name: String,
    pub seconds: f64,
    pub others_cores: Option<f64>,
    /// How far the counters' rounding may put `others_cores` off, in logical CPUs.
    pub others_cores_error: Option<f64>,
    pub own_cores: Option<f64>,
    pub gpu_busy_percent: Option<f64>,
    /// How many readings of `gpu_busy_percent` the part's mean took: its end's, and one each
    /// second during it.
    pub gpu_readings: usize,
    pub own_gpu_percent: Option<f64>,
    pub others_gpu_percent: Option<f64>,
    pub performance_levels: Vec<String>,
    pub sclk_mhz: Option<(u32, u32)>,
    pub mclk_mhz: Option<(u32, u32)>,
    pub disturbed: bool,
    pub each_second: Vec<SecondRead>,
}

/// One reading of the sampler's, between it and the one before.
pub struct SecondRead {
    pub seconds_since_start: f64,
    pub others_cores: Option<f64>,
    /// How far the counters' rounding may put `others_cores` off over this reading's interval.
    pub others_cores_error: Option<f64>,
    pub gpu_busy_percent: Option<f64>,
    /// `gpu_busy_percent` less the prototype's own share of the graphics engine over the
    /// interval.
    pub others_gpu_percent: Option<f64>,
    pub sclk_mhz: Option<u32>,
    pub mclk_mhz: Option<u32>,
}

impl LoadWindow {
    fn of(name: String, samples: &[Sample], rounding_ticks: f64) -> Self {
        let (Some(first), Some(last)) = (samples.first(), samples.last()) else {
            return Self::empty(name);
        };
        let seconds = last.at.duration_since(first.at).as_secs_f64();
        // Others' and the prototype's logical CPUs, and the rounding's bound, all in ticks
        // divided by the ticks each CPU lived through.
        let cores = |from: &Sample, to: &Sample| -> Option<(f64, f64, f64)> {
            let (busy_from, total_from, cpus) = from.cpu?;
            let (busy_to, total_to, _) = to.cpu?;
            let own = to.own_ticks?.checked_sub(from.own_ticks?)? as f64;
            let elapsed_per_cpu = total_to.checked_sub(total_from)? as f64 / f64::from(cpus);
            if elapsed_per_cpu <= 0.0 {
                return None;
            }
            let busy = busy_to.checked_sub(busy_from)? as f64;
            Some((
                ((busy - own) / elapsed_per_cpu).max(0.0),
                own / elapsed_per_cpu,
                rounding_ticks / elapsed_per_cpu,
            ))
        };
        let window_cores = cores(first, last);
        // The first sample is read as the part begins and shows the GPU before it: left out.
        let during = if samples.len() > 1 {
            &samples[1..]
        } else {
            samples
        };
        let busy: Vec<f64> = during
            .iter()
            .filter_map(|sample| sample.gpu_busy_percent)
            .collect();
        let gpu_busy_percent =
            (!busy.is_empty()).then(|| busy.iter().sum::<f64>() / busy.len() as f64);
        let own_gpu_percent = match (first.own_gpu_ns, last.own_gpu_ns) {
            (Some(from), Some(to)) if seconds > 0.0 => {
                Some(to.saturating_sub(from) as f64 / (seconds * 1e9) * 100.0)
            }
            _ => None,
        };
        let others_gpu_percent =
            gpu_busy_percent.map(|busy| (busy - own_gpu_percent.unwrap_or(0.0)).max(0.0));
        let mut performance_levels: Vec<String> = Vec::new();
        for level in samples
            .iter()
            .filter_map(|sample| sample.performance_level.clone())
        {
            if !performance_levels.contains(&level) {
                performance_levels.push(level);
            }
        }
        let range = |values: Vec<u32>| Some((*values.iter().min()?, *values.iter().max()?));
        let each_second = samples
            .windows(2)
            .map(|pair| {
                let interval_cores = cores(&pair[0], &pair[1]);
                let seconds = pair[1].at.duration_since(pair[0].at).as_secs_f64();
                let own_gpu = match (pair[0].own_gpu_ns, pair[1].own_gpu_ns) {
                    (Some(from), Some(to)) if seconds > 0.0 => {
                        to.saturating_sub(from) as f64 / (seconds * 1e9) * 100.0
                    }
                    _ => 0.0,
                };
                SecondRead {
                    seconds_since_start: pair[1].at.duration_since(first.at).as_secs_f64(),
                    others_cores: interval_cores.map(|(others, _, _)| others),
                    others_cores_error: interval_cores.map(|(_, _, error)| error),
                    gpu_busy_percent: pair[1].gpu_busy_percent,
                    others_gpu_percent: pair[1]
                        .gpu_busy_percent
                        .map(|busy| (busy - own_gpu).max(0.0)),
                    sclk_mhz: pair[1].sclk_mhz,
                    mclk_mhz: pair[1].mclk_mhz,
                }
            })
            .collect();
        let others_cores = window_cores.map(|(others, _, _)| others);
        Self {
            name,
            seconds,
            others_cores,
            others_cores_error: window_cores.map(|(_, _, error)| error),
            own_cores: window_cores.map(|(_, own, _)| own),
            gpu_busy_percent,
            gpu_readings: busy.len(),
            own_gpu_percent,
            others_gpu_percent,
            performance_levels,
            sclk_mhz: range(
                samples
                    .iter()
                    .filter_map(|sample| sample.sclk_mhz)
                    .collect(),
            ),
            mclk_mhz: range(
                samples
                    .iter()
                    .filter_map(|sample| sample.mclk_mhz)
                    .collect(),
            ),
            disturbed: others_cores.is_some_and(|cores| cores > OTHERS_CORES_LIMIT)
                || others_gpu_percent.is_some_and(|percent| percent > OTHERS_GPU_PERCENT_LIMIT),
            each_second,
        }
    }

    /// Others' load in the part's busiest second: the most logical CPUs of any reading, less the
    /// counters' rounding over its interval, and the most of the GPU.
    pub fn busiest_second(&self) -> (Option<f64>, Option<f64>) {
        let most = |values: Vec<f64>| values.into_iter().reduce(f64::max);
        (
            most(
                self.each_second
                    .iter()
                    .filter_map(|read| {
                        Some((read.others_cores? - read.others_cores_error?).max(0.0))
                    })
                    .collect(),
            ),
            most(
                self.each_second
                    .iter()
                    .filter_map(|read| read.others_gpu_percent)
                    .collect(),
            ),
        )
    }

    /// Disturbed on average, or in its busiest second beyond the counters' rounding.
    pub fn disturbed_in_a_second(&self) -> bool {
        let (cores, gpu) = self.busiest_second();
        self.disturbed
            || cores.is_some_and(|cores| cores > OTHERS_CORES_LIMIT)
            || gpu.is_some_and(|percent| percent > OTHERS_GPU_PERCENT_LIMIT)
    }

    fn empty(name: String) -> Self {
        Self {
            name,
            seconds: 0.0,
            others_cores: None,
            others_cores_error: None,
            own_cores: None,
            gpu_busy_percent: None,
            gpu_readings: 0,
            own_gpu_percent: None,
            others_gpu_percent: None,
            performance_levels: Vec::new(),
            sclk_mhz: None,
            mclk_mhz: None,
            disturbed: false,
            each_second: Vec::new(),
        }
    }

    pub fn to_json(&self) -> Json {
        let number = |value: Option<f64>| Json::optional(value, Json::Number);
        let clock = |value: Option<u32>| Json::optional(value, |mhz| Json::Integer(i64::from(mhz)));
        let range = |value: Option<(u32, u32)>| {
            Json::optional(value, |(low, high)| {
                Json::Array(vec![
                    Json::Integer(i64::from(low)),
                    Json::Integer(i64::from(high)),
                ])
            })
        };
        Json::Object(vec![
            ("part", Json::text(&self.name)),
            ("seconds", Json::Number(self.seconds)),
            ("others_logical_cpus", number(self.others_cores)),
            (
                "others_logical_cpus_rounding_bound",
                number(self.others_cores_error),
            ),
            ("own_logical_cpus", number(self.own_cores)),
            ("gpu_busy_percent", number(self.gpu_busy_percent)),
            ("gpu_busy_readings", Json::Integer(self.gpu_readings as i64)),
            ("own_gfx_percent", number(self.own_gpu_percent)),
            ("others_gpu_percent", number(self.others_gpu_percent)),
            (
                "power_dpm_force_performance_level",
                if self.performance_levels.is_empty() {
                    Json::Null
                } else {
                    Json::Array(self.performance_levels.iter().map(Json::text).collect())
                },
            ),
            ("sclk_mhz_range", range(self.sclk_mhz)),
            ("mclk_mhz_range", range(self.mclk_mhz)),
            ("disturbed", Json::Bool(self.disturbed)),
            (
                "each_second",
                Json::Array(
                    self.each_second
                        .iter()
                        .map(|read| {
                            Json::Object(vec![
                                ("at_seconds", Json::Number(read.seconds_since_start)),
                                ("others_logical_cpus", number(read.others_cores)),
                                (
                                    "others_logical_cpus_rounding_bound",
                                    number(read.others_cores_error),
                                ),
                                ("gpu_busy_percent", number(read.gpu_busy_percent)),
                                ("others_gpu_percent", number(read.others_gpu_percent)),
                                ("sclk_mhz", clock(read.sclk_mhz)),
                                ("mclk_mhz", clock(read.mclk_mhz)),
                            ])
                        })
                        .collect(),
                ),
            ),
        ])
    }
}

pub fn to_markdown(windows: &[LoadWindow], out: &mut String) {
    out.push_str(&format!(
        "Other programs' load during each timed part (ADR-0060), an estimate. Their CPU comes \
         from counters in 10 ms ticks; ± is how far the counters' rounding may put it off. Their \
         GPU is `gpu_busy_percent`, the graphics engine's activity, averaged over its readings \
         (the part's end and one each second, so one or two for a part shorter than a second), \
         less the prototype's own share of the graphics engine (`drm-engine-gfx`). Disturbed: others \
         above {OTHERS_CORES_LIMIT} logical CPUs or {OTHERS_GPU_PERCENT_LIMIT} % of the GPU on \
         average.\n\n"
    ));
    out.push_str(
        "| Part | Seconds | Others' CPUs | ± | Own CPUs | GPU busy % | Readings | Own gfx % | Others' GPU % | Level | sclk MHz | mclk MHz | Disturbed |\n\
         |---|---|---|---|---|---|---|---|---|---|---|---|---|\n",
    );
    let number =
        |value: Option<f64>| value.map_or_else(|| "-".to_owned(), |value| format!("{value:.2}"));
    let range = |value: Option<(u32, u32)>| {
        value.map_or_else(
            || "-".to_owned(),
            |(low, high)| {
                if low == high {
                    low.to_string()
                } else {
                    format!("{low}-{high}")
                }
            },
        )
    };
    for window in windows {
        out.push_str(&format!(
            "| {} | {:.1} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} |\n",
            window.name,
            window.seconds,
            number(window.others_cores),
            number(window.others_cores_error),
            number(window.own_cores),
            number(window.gpu_busy_percent),
            window.gpu_readings,
            number(window.own_gpu_percent),
            number(window.others_gpu_percent),
            if window.performance_levels.is_empty() {
                "-".to_owned()
            } else {
                window.performance_levels.join(", ")
            },
            range(window.sclk_mhz),
            range(window.mclk_mhz),
            if window.disturbed { "**yes**" } else { "no" },
        ));
    }
    out.push('\n');
}

/// The machine's own facts.
pub struct Machine {
    pub kernel: Option<String>,
    pub cpu_model: Option<String>,
    pub logical_cpus: Option<u32>,
    pub memory_bytes: Option<u64>,
}

impl Machine {
    pub fn read() -> Self {
        let cpuinfo = fs::read_to_string("/proc/cpuinfo").unwrap_or_default();
        Self {
            kernel: fs::read_to_string("/proc/sys/kernel/osrelease")
                .ok()
                .map(|text| text.trim().to_owned()),
            cpu_model: cpuinfo
                .lines()
                .find_map(|line| line.strip_prefix("model name"))
                .and_then(|rest| rest.split_once(':'))
                .map(|(_, model)| model.trim().to_owned()),
            logical_cpus: machine_cpu_ticks().map(|(_, _, cpus)| cpus),
            memory_bytes: fs::read_to_string("/proc/meminfo").ok().and_then(|text| {
                text.lines()
                    .find_map(|line| line.strip_prefix("MemTotal:"))
                    .and_then(|rest| rest.trim().strip_suffix("kB"))
                    .and_then(|kib| kib.trim().parse::<u64>().ok())
                    .map(|kib| kib * 1024)
            }),
        }
    }

    pub fn to_json(&self) -> Json {
        Json::Object(vec![
            ("kernel", Json::optional(self.kernel.as_ref(), Json::text)),
            (
                "cpu_model",
                Json::optional(self.cpu_model.as_ref(), Json::text),
            ),
            (
                "logical_cpus",
                Json::optional(self.logical_cpus, |cpus| Json::Integer(i64::from(cpus))),
            ),
            (
                "memory_bytes",
                Json::optional(self.memory_bytes, |bytes| Json::Integer(bytes as i64)),
            ),
        ])
    }

    pub fn to_markdown(&self, out: &mut String) {
        let text = |value: &Option<String>| value.clone().unwrap_or_else(|| "unknown".to_owned());
        out.push_str(&format!("- Kernel: {}\n", text(&self.kernel)));
        out.push_str(&format!("- CPU: {}", text(&self.cpu_model)));
        if let Some(cpus) = self.logical_cpus {
            out.push_str(&format!(", {cpus} logical CPUs"));
        }
        out.push('\n');
        match self.memory_bytes {
            Some(bytes) => out.push_str(&format!(
                "- Memory: {:.1} GiB\n\n",
                bytes as f64 / f64::from(1u32 << 30)
            )),
            None => out.push_str("- Memory: unknown\n\n"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        LoadWindow, Sample, cpu_rounding_ticks, current_clock, parse_fdinfo, parse_own_stat,
        parse_proc_stat,
    };
    use std::time::{Duration, Instant};

    /// A sample `second` seconds in, of 16 CPUs, `busy` ticks busy of `total`, the GPU `gpu` %
    /// busy with none of it the prototype's.
    fn sample(start: Instant, second: u64, busy: u64, total: u64, gpu: Option<f64>) -> Sample {
        Sample {
            at: start + Duration::from_secs(second),
            cpu: Some((busy, total, 16)),
            own_ticks: Some(0),
            gpu_busy_percent: gpu,
            own_gpu_ns: Some(0),
            performance_level: None,
            sclk_mhz: None,
            mclk_mhz: None,
        }
    }

    #[test]
    fn a_busy_second_disturbs_a_quiet_average() {
        let start = Instant::now();
        // Others: 0.5 logical CPUs in the first second, 3 in the second: 1.75 on average.
        let cpu = [
            sample(start, 0, 0, 0, None),
            sample(start, 1, 50, 1600, None),
            sample(start, 2, 350, 3200, None),
        ];
        let window = LoadWindow::of("cpu".to_owned(), &cpu, cpu_rounding_ticks(1));
        assert!(!window.disturbed);
        let (cores, _) = window.busiest_second();
        assert!((cores.expect("a reading") - 2.92).abs() < 1e-9);
        assert!(window.disturbed_in_a_second());
        // The GPU: 5 % then 15 %, 10 % on average, which is not above the limit.
        let gpu = [
            sample(start, 0, 0, 0, Some(0.0)),
            sample(start, 1, 0, 1600, Some(5.0)),
            sample(start, 2, 0, 3200, Some(15.0)),
        ];
        let window = LoadWindow::of("gpu".to_owned(), &gpu, cpu_rounding_ticks(1));
        assert!(!window.disturbed);
        assert!(window.disturbed_in_a_second());
        let quiet = [
            sample(start, 0, 0, 0, Some(1.0)),
            sample(start, 1, 100, 1600, Some(2.0)),
        ];
        let window = LoadWindow::of("quiet".to_owned(), &quiet, cpu_rounding_ticks(2));
        assert!(!window.disturbed_in_a_second());
    }

    #[test]
    fn reads_the_kernels_files() {
        let stat = "cpu  10 1 5 100 4 2 3 0 0 0\ncpu0 5 0 2 50 2 1 1 0 0 0\ncpu1 5 1 3 50 2 1 2 0 0 0\nintr 1\n";
        assert_eq!(parse_proc_stat(stat), Some((21, 125, 2)));
        let own = "42 (a (b) c) R 1 2 3 4 5 6 7 8 9 10 70 30 0 0";
        assert_eq!(parse_own_stat(own), Some(100));
        let fdinfo = "drm-client-id:\t7\ndrm-engine-gfx:\t1500 ns\ndrm-engine-compute:\t500 ns\ndrm-engine-capacity-gfx:\t2\n";
        assert_eq!(parse_fdinfo(fdinfo), (Some(7), 1500));
        assert_eq!(
            current_clock("0: 500Mhz \n1: 2530Mhz *\n2: 2660Mhz \n"),
            Some(2530)
        );
        assert_eq!(current_clock("0: 500Mhz \n"), None);
    }
}
