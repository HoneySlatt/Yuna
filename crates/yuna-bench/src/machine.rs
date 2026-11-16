//! The machine a run measures on: what it is (its kernel, processor, GPU and firmware, the Mesa of
//! the shell), and its state each second while a window is measured: what its other programs
//! use, the GPU's busy share, clocks, performance level and power, the battery's power when the
//! machine runs on it, each screen's state, each core's frequency and the temperatures.
//!
//! Everything is read from `/proc` and `/sys` without root. A file that is missing or unreadable
//! gives `None`, written `null`, never 0: amdgpu gives `power1_average`, a laptop's APU does not.
//! Each reader of `/sys` takes its root, so that tests read a made-up one.
//!
//! The other programs' load follows
//! [ADR-0060](../../../docs/adr/0060-development-first-agents-in-their-own-yuna.md): their
//! processor time is the machine's busy time less the bench's and its descendants', every
//! compositor and client it starts among them; their GPU time is the DRM statistics of every
//! other process on the drawing engines, `gfx` and `compute`. A window whose other programs used
//! more than [`OTHERS_CPU_LIMIT`] cores or [`OTHERS_GPU_LIMIT`] % of that GPU time, on average,
//! is *disturbed*.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::drm;
use crate::procfs::{self, Proc};

/// How often the machine's state is read.
pub const PERIOD: Duration = Duration::from_secs(1);
/// Above this many cores on average, the other programs disturb a window.
pub const OTHERS_CPU_LIMIT: f64 = 2.0;
/// Above this share of the drawing engines' time on average, in percent, they disturb it.
pub const OTHERS_GPU_LIMIT: f64 = 10.0;
/// The engines a compositor draws on, as the DRM statistics name them: another program's video
/// decoding, on engines of its own, takes nothing from them.
pub const DRAWING_ENGINES: [&str; 2] = ["gfx", "compute"];
/// The kernel's clock ticks a second in `/proc/stat` and `/proc/<pid>/stat`, `USER_HZ`: 100 on
/// x86_64, the only machines Yuna runs on (ADR-0006).
const TICKS_A_SECOND: f64 = 100.0;

/// What the machine is.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct MachineInfo {
    /// The kernel's release, `/proc/sys/kernel/osrelease`.
    pub kernel: Option<String>,
    /// The processor's model name.
    pub cpu_model: Option<String>,
    /// How many processors the kernel runs, hardware threads included.
    pub logical_cpus: Option<usize>,
    /// The machine's memory, in KiB.
    pub memory_kib: Option<u64>,
    /// The GPU behind the render node the compositors draw on.
    pub gpu: Option<GpuInfo>,
    /// The shell's Mesa, its store folder and its version, which every compositor is given.
    pub mesa_folder: Option<String>,
    /// Its version.
    pub mesa_version: Option<String>,
}

/// The GPU behind a render node, from its sysfs folder.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct GpuInfo {
    /// The render node, such as `/dev/dri/renderD128`.
    pub render_node: String,
    /// Its PCI slot.
    pub pci_slot: Option<String>,
    /// Its kernel driver, such as `amdgpu`.
    pub driver: Option<String>,
    /// Its PCI vendor and device ids, and its revision.
    pub vendor_id: Option<String>,
    /// Its PCI device id.
    pub device_id: Option<String>,
    /// Its PCI revision.
    pub revision: Option<String>,
    /// Its video BIOS's version.
    pub vbios: Option<String>,
    /// Each firmware's version, by the name amdgpu's `fw_version/` gives it.
    pub firmware: BTreeMap<String, String>,
}

impl MachineInfo {
    /// Reads what the machine is; `render_node` names the GPU, `mesa_folder` the shell's Mesa.
    pub fn read(render_node: &str, mesa_folder: Option<&str>) -> MachineInfo {
        let cpuinfo = read_text("/proc/cpuinfo").unwrap_or_default();
        MachineInfo {
            kernel: read_text("/proc/sys/kernel/osrelease"),
            cpu_model: cpuinfo.lines().find_map(|line| {
                let (key, value) = line.split_once(':')?;
                (key.trim() == "model name").then(|| value.trim().to_owned())
            }),
            logical_cpus: Some(
                cpuinfo
                    .lines()
                    .filter(|line| line.starts_with("processor"))
                    .count(),
            )
            .filter(|&count| count > 0),
            memory_kib: read_text("/proc/meminfo")
                .and_then(|meminfo| procfs::parse_kib(&meminfo, "MemTotal")),
            gpu: GpuInfo::read(Path::new(SYS), render_node),
            mesa_folder: mesa_folder.map(str::to_owned),
            mesa_version: mesa_folder
                .and_then(procfs::store_version)
                .map(str::to_owned),
        }
    }
}

impl GpuInfo {
    fn read(sys: &Path, render_node: &str) -> Option<GpuInfo> {
        let device = gpu_device(sys, render_node)?;
        let firmware = fs::read_dir(device.join("fw_version"))
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|file| {
                let name = file.file_name().to_str()?.to_owned();
                Some((name, read_text(file.path())?))
            })
            .collect();
        Some(GpuInfo {
            render_node: render_node.to_owned(),
            pci_slot: device.file_name().map(|n| n.to_string_lossy().into_owned()),
            driver: fs::read_link(device.join("driver"))
                .ok()
                .and_then(|driver| Some(driver.file_name()?.to_string_lossy().into_owned())),
            vendor_id: read_text(device.join("vendor")),
            device_id: read_text(device.join("device")),
            revision: read_text(device.join("revision")),
            vbios: read_text(device.join("vbios_version")),
            firmware,
        })
    }
}

/// The sysfs folder of the device behind a node such as `/dev/dri/renderD128`.
fn gpu_device(sys: &Path, render_node: &str) -> Option<PathBuf> {
    let name = Path::new(render_node).file_name()?;
    fs::canonicalize(sys.join("class/drm").join(name).join("device")).ok()
}

/// A file's text, trimmed; none if it cannot be read.
fn read_text(path: impl AsRef<Path>) -> Option<String> {
    fs::read_to_string(path)
        .ok()
        .map(|text| text.trim().to_owned())
        .filter(|text| !text.is_empty())
}

/// The machine's state at one reading.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct MachineState {
    /// The GPU's busy share, `gpu_busy_percent`, everyone's work included, in percent.
    pub gpu_busy_percent: Option<u32>,
    /// The GPU's shader clock now, the `*` line of `pp_dpm_sclk`, in MHz.
    pub sclk_mhz: Option<u32>,
    /// Its memory clock now, from `pp_dpm_mclk`, in MHz.
    pub mclk_mhz: Option<u32>,
    /// `power_dpm_force_performance_level`: `auto`, `high`, …
    pub performance_level: Option<String>,
    /// The GPU's power, `power1_average` of its hwmon, in watts.
    pub power_w: Option<f64>,
    /// Whether the machine runs on its battery: a battery discharges. None without a battery.
    #[serde(default)]
    pub on_battery: Option<bool>,
    /// The power the batteries give, in watts, while they discharge: the whole machine's draw.
    /// None on mains, or without a battery ([`battery`]).
    #[serde(default)]
    pub battery_w: Option<f64>,
    /// Each connected screen, by its connector (`card1-DP-1`), as the kernel tells it.
    #[serde(default)]
    pub screens: BTreeMap<String, Screen>,
    /// Each processor's frequency, `scaling_cur_freq`, in MHz, by its number.
    pub cpu_mhz: BTreeMap<u32, Option<f64>>,
    /// Each temperature of every hwmon, by `<hwmon name> <label>`, in degrees Celsius.
    pub temperatures_celsius: BTreeMap<String, Option<f64>>,
}

/// A connected screen, from its connector's folder in `/sys/class/drm`. The kernel gives no
/// screen's current mode there: its preferred one, the first of `modes`, is what a compositor
/// picks by default, and the refresh it really runs at comes from the presentation feedback.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Screen {
    /// `enabled`: whether a compositor drives it.
    pub enabled: Option<String>,
    /// `dpms`: `On` or `Off`.
    pub dpms: Option<String>,
    /// Its preferred mode, such as `3840x2160`.
    pub preferred_mode: Option<String>,
}

impl MachineState {
    /// Reads the state now, of the GPU behind `render_node`.
    pub fn read(render_node: &str) -> MachineState {
        MachineState::read_at(Path::new(SYS), render_node)
    }

    /// Reads the state now from the sysfs mounted at `sys`.
    pub fn read_at(sys: &Path, render_node: &str) -> MachineState {
        let device = gpu_device(sys, render_node);
        let gpu_file = |name: &str| device.as_ref().and_then(|d| read_text(d.join(name)));
        let gpu_hwmon = device.as_ref().and_then(|d| {
            fs::read_dir(d.join("hwmon"))
                .ok()?
                .flatten()
                .next()
                .map(|e| e.path())
        });
        let (on_battery, battery_w) = battery(sys);
        MachineState {
            gpu_busy_percent: gpu_file("gpu_busy_percent").and_then(|v| v.parse().ok()),
            sclk_mhz: gpu_file("pp_dpm_sclk").and_then(|v| parse_current_clock(&v)),
            mclk_mhz: gpu_file("pp_dpm_mclk").and_then(|v| parse_current_clock(&v)),
            performance_level: gpu_file("power_dpm_force_performance_level"),
            power_w: gpu_hwmon
                .and_then(|h| read_text(h.join("power1_average")))
                .and_then(|microwatts| microwatts.parse::<f64>().ok())
                .map(|microwatts| microwatts / 1e6),
            on_battery,
            battery_w,
            screens: screens(sys),
            cpu_mhz: cpu_frequencies(sys),
            temperatures_celsius: temperatures(sys),
        }
    }
}

/// Where sysfs is mounted.
pub const SYS: &str = "/sys";

/// Whether the machine runs on its battery, and the power its batteries give then, in watts:
/// each discharging battery's `power_now`, or `current_now × voltage_now` where the battery
/// gives no power (the Steam Deck's does not). A charging or full battery gives none: its
/// figures are the charger's, not the machine's draw. A device's battery, a wireless mouse's
/// (`scope` `Device`), is not the machine's.
pub fn battery(sys: &Path) -> (Option<bool>, Option<f64>) {
    let mut batteries = 0;
    let mut watts: Option<f64> = None;
    let mut discharging = false;
    for supply in fs::read_dir(sys.join("class/power_supply"))
        .into_iter()
        .flatten()
        .flatten()
    {
        let folder = supply.path();
        if read_text(folder.join("type")).as_deref() != Some("Battery")
            || read_text(folder.join("scope")).as_deref() == Some("Device")
        {
            continue;
        }
        batteries += 1;
        if read_text(folder.join("status")).as_deref() != Some("Discharging") {
            continue;
        }
        discharging = true;
        let number = |name: &str| read_text(folder.join(name))?.parse::<f64>().ok();
        let power = number("power_now")
            .map(|microwatts| microwatts / 1e6)
            .or_else(|| Some(number("current_now")? * number("voltage_now")? / 1e12));
        // Some batteries report their draw as a negative current.
        if let Some(power) = power {
            *watts.get_or_insert(0.0) += power.abs();
        }
    }
    match batteries {
        0 => (None, None),
        _ => (Some(discharging), watts),
    }
}

/// Each connected screen, by its connector's folder name.
pub fn screens(sys: &Path) -> BTreeMap<String, Screen> {
    let mut screens = BTreeMap::new();
    for entry in fs::read_dir(sys.join("class/drm"))
        .into_iter()
        .flatten()
        .flatten()
    {
        let name = entry.file_name().to_string_lossy().into_owned();
        // Connectors are `card<n>-<connector>`; the cards and render nodes have no dash.
        if !(name.starts_with("card") && name.contains('-')) {
            continue;
        }
        let folder = entry.path();
        if read_text(folder.join("status")).as_deref() != Some("connected") {
            continue;
        }
        screens.insert(
            name,
            Screen {
                enabled: read_text(folder.join("enabled")),
                dpms: read_text(folder.join("dpms")),
                preferred_mode: read_text(folder.join("modes"))
                    .and_then(|modes| modes.lines().next().map(str::to_owned)),
            },
        );
    }
    screens
}

/// A GPU's performance level, `power_dpm_force_performance_level`, which the tour needs at
/// `auto` (ADR-0071).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GpuLevel {
    /// The level's file.
    pub file: PathBuf,
    /// Its value: `auto`, `high`, `manual`, …
    pub level: String,
}

/// The performance level of every GPU that has one, in the order of their cards.
pub fn gpu_levels(sys: &Path) -> Vec<GpuLevel> {
    let mut cards: Vec<PathBuf> = fs::read_dir(sys.join("class/drm"))
        .into_iter()
        .flatten()
        .flatten()
        .filter(|entry| {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            name.strip_prefix("card").is_some_and(|number| {
                !number.is_empty() && number.chars().all(|c| c.is_ascii_digit())
            })
        })
        .map(|entry| entry.path())
        .collect();
    cards.sort();
    cards
        .into_iter()
        .filter_map(|card| {
            let file = card.join("device/power_dpm_force_performance_level");
            let level = read_text(&file)?;
            Some(GpuLevel { file, level })
        })
        .collect()
}

/// The current level of a `pp_dpm_*` file, the line marked `*`, in MHz: `1: 500Mhz *` gives 500.
pub fn parse_current_clock(text: &str) -> Option<u32> {
    text.lines()
        .find(|line| line.trim_end().ends_with('*'))
        .and_then(|line| line.split_once(':'))
        .and_then(|(_, value)| {
            let value = value.trim().trim_end_matches('*').trim();
            value
                .strip_suffix("Mhz")
                .or_else(|| value.strip_suffix("MHz"))?
                .parse()
                .ok()
        })
}

fn cpu_frequencies(sys: &Path) -> BTreeMap<u32, Option<f64>> {
    fs::read_dir(sys.join("devices/system/cpu"))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name();
            let number = name.to_str()?.strip_prefix("cpu")?.parse().ok()?;
            let khz = read_text(entry.path().join("cpufreq/scaling_cur_freq"))
                .and_then(|khz| khz.parse::<f64>().ok());
            Some((number, khz.map(|khz| khz / 1000.0)))
        })
        .collect()
}

fn temperatures(sys: &Path) -> BTreeMap<String, Option<f64>> {
    let mut temperatures = BTreeMap::new();
    for hwmon in fs::read_dir(sys.join("class/hwmon"))
        .into_iter()
        .flatten()
        .flatten()
    {
        let folder = hwmon.path();
        let device = read_text(folder.join("name")).unwrap_or_else(|| "?".to_owned());
        for file in fs::read_dir(&folder).into_iter().flatten().flatten() {
            let file_name = file.file_name();
            let Some(sensor) = file_name
                .to_str()
                .and_then(|name| name.strip_suffix("_input"))
                .filter(|name| name.starts_with("temp"))
            else {
                continue;
            };
            let label = read_text(folder.join(format!("{sensor}_label")))
                .unwrap_or_else(|| sensor.to_owned());
            let celsius = read_text(file.path())
                .and_then(|millidegrees| millidegrees.parse::<f64>().ok())
                .map(|millidegrees| millidegrees / 1000.0);
            temperatures.insert(format!("{device} {label}"), celsius);
        }
    }
    temperatures
}

/// One reading of what the machine has done so far, for the other programs' load.
#[derive(Debug, Clone, PartialEq)]
struct Reading {
    at: Instant,
    /// Every processor's busy time, in clock ticks.
    busy_ticks: Option<u64>,
    /// The bench's and its descendants' processor time, in clock ticks, their reaped children's
    /// included.
    own_ticks: u64,
    /// Each other process's GPU time on the drawing engines, in nanoseconds; those this user may
    /// not read are missing.
    others_gpu_ns: BTreeMap<u32, u64>,
    state: MachineState,
}

impl Reading {
    fn take(proc: &Proc, render_node: &str) -> Reading {
        let processes: Vec<(u32, u32, u64)> = proc
            .pids()
            .into_iter()
            .filter_map(|pid| {
                let (parent, ticks) = proc.stat(pid)?;
                Some((pid, parent, ticks))
            })
            .collect();
        let own = descendants(std::process::id(), &processes);
        let own_ticks = processes
            .iter()
            .filter(|(pid, _, _)| own.contains(pid))
            .map(|(_, _, ticks)| ticks)
            .sum();
        let others_gpu_ns = processes
            .iter()
            .filter(|(pid, _, _)| !own.contains(pid))
            .filter_map(|(pid, _, _)| {
                let files = drm::drm_files(proc, &[*pid]);
                if files.is_empty() {
                    return None;
                }
                let engines = drm::engines(&files);
                let drawing = DRAWING_ENGINES
                    .iter()
                    .filter_map(|engine| engines.get(*engine))
                    .sum();
                Some((*pid, drawing))
            })
            .collect();
        Reading {
            at: Instant::now(),
            busy_ticks: fs::read_to_string("/proc/stat")
                .ok()
                .and_then(|stat| parse_busy_ticks(&stat)),
            own_ticks,
            others_gpu_ns,
            state: MachineState::read(render_node),
        }
    }
}

/// `root` and its descendants, of `processes` given as `(pid, parent, ticks)`.
fn descendants(root: u32, processes: &[(u32, u32, u64)]) -> BTreeSet<u32> {
    let mut own = BTreeSet::from([root]);
    // Parents come before their children in no set order: walk until nothing is added.
    let mut grew = true;
    while grew {
        grew = false;
        for (pid, parent, _) in processes {
            if own.contains(parent) && own.insert(*pid) {
                grew = true;
            }
        }
    }
    own
}

/// The busy time of `/proc/stat`'s first line: user, nice, system, irq, softirq and steal, its
/// idle time and its wait for disks left out.
pub fn parse_busy_ticks(stat: &str) -> Option<u64> {
    let fields: Vec<u64> = stat
        .lines()
        .next()?
        .strip_prefix("cpu ")?
        .split_whitespace()
        .map(|field| field.parse().ok())
        .collect::<Option<_>>()?;
    let [user, nice, system, _idle, _iowait, irq, softirq, steal, ..] = fields[..] else {
        return None;
    };
    Some(user + nice + system + irq + softirq + steal)
}

/// The machine over one second of a window, as the run's file keeps it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Second {
    /// Its end, in seconds from the window's start.
    pub at_s: f64,
    /// The other programs' processor time over it, in cores.
    pub others_cpu_cores: Option<f64>,
    /// Their share of the drawing engines' time, in percent.
    pub others_gpu_percent: Option<f64>,
    /// The bench's own and its descendants', in cores.
    pub bench_cpu_cores: f64,
    /// The machine's state at its end.
    pub state: MachineState,
}

/// What the other programs used over a window.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Load {
    /// Their processor time on average, in cores; none if `/proc/stat` could not be read.
    pub others_cpu_cores_mean: Option<f64>,
    /// At most over one period between two readings.
    pub others_cpu_cores_max: Option<f64>,
    /// Their share of the drawing engines' time on average, in percent; none when no other
    /// process's GPU files could be read: unknown, not nil.
    pub others_gpu_percent_mean: Option<f64>,
    /// At most over one period.
    pub others_gpu_percent_max: Option<f64>,
    /// The most other processes whose GPU files one reading could read.
    pub others_gpu_processes: usize,
    /// The bench's own and its descendants' processor time, in cores, on average.
    pub bench_cpu_cores_mean: f64,
    /// Whether the other programs disturbed the window, by the rule above; none if neither of
    /// their loads could be read.
    pub disturbed: Option<bool>,
    /// Each second.
    pub seconds: Vec<Second>,
}

impl Load {
    /// The load of `readings`, which hold two at least, in order.
    fn of(readings: &[Reading]) -> Load {
        let (first, last) = (&readings[0], &readings[readings.len() - 1]);
        let seconds = |from: &Reading, to: &Reading| (to.at - from.at).as_secs_f64().max(1e-3);
        let own = |from: &Reading, to: &Reading| {
            to.own_ticks.saturating_sub(from.own_ticks) as f64 / TICKS_A_SECOND / seconds(from, to)
        };
        let others_cpu = |from: &Reading, to: &Reading| {
            let busy = to.busy_ticks?.saturating_sub(from.busy_ticks?);
            let own = to.own_ticks.saturating_sub(from.own_ticks);
            Some(busy.saturating_sub(own) as f64 / TICKS_A_SECOND / seconds(from, to))
        };
        let others_gpu = |from: &Reading, to: &Reading| {
            if to.others_gpu_ns.is_empty() {
                return None;
            }
            let mut used = 0;
            for (pid, after) in &to.others_gpu_ns {
                // A process that started between the readings counts from the next one.
                if let Some(before) = from.others_gpu_ns.get(pid) {
                    used += after.saturating_sub(*before);
                }
            }
            Some(used as f64 / 1e9 / seconds(from, to) * 100.0)
        };
        let max = |values: Vec<Option<f64>>| values.into_iter().flatten().reduce(f64::max);
        let pairs: Vec<(&Reading, &Reading)> = readings
            .windows(2)
            .map(|pair| (&pair[0], &pair[1]))
            .collect();
        let others_cpu_mean = others_cpu(first, last);
        let others_gpu_mean = others_gpu(first, last);
        let disturbed = match (others_cpu_mean, others_gpu_mean) {
            (None, None) => None,
            (cpu, gpu) => Some(
                cpu.is_some_and(|c| c > OTHERS_CPU_LIMIT)
                    || gpu.is_some_and(|g| g > OTHERS_GPU_LIMIT),
            ),
        };
        Load {
            others_cpu_cores_mean: others_cpu_mean,
            others_cpu_cores_max: max(pairs.iter().map(|(a, b)| others_cpu(a, b)).collect()),
            others_gpu_percent_mean: others_gpu_mean,
            others_gpu_percent_max: max(pairs.iter().map(|(a, b)| others_gpu(a, b)).collect()),
            others_gpu_processes: readings
                .iter()
                .map(|reading| reading.others_gpu_ns.len())
                .max()
                .unwrap_or(0),
            bench_cpu_cores_mean: own(first, last),
            disturbed,
            seconds: pairs
                .iter()
                .map(|(from, to)| Second {
                    at_s: (to.at - first.at).as_secs_f64(),
                    others_cpu_cores: others_cpu(from, to),
                    others_gpu_percent: others_gpu(from, to),
                    bench_cpu_cores: own(from, to),
                    state: to.state.clone(),
                })
                .collect(),
        }
    }
}

/// The machine read every [`PERIOD`] over a window, on a thread of its own.
pub struct MachineWatch {
    readings: Arc<Mutex<Vec<Reading>>>,
    stop: Arc<AtomicBool>,
    thread: std::thread::JoinHandle<()>,
    render_node: String,
}

impl MachineWatch {
    /// Starts reading the machine, the GPU behind `render_node` for its state.
    pub fn start(render_node: &str) -> MachineWatch {
        let proc = Proc::system();
        let readings = Arc::new(Mutex::new(vec![Reading::take(&proc, render_node)]));
        let stop = Arc::new(AtomicBool::new(false));
        let thread = {
            let (readings, stop, node) = (
                Arc::clone(&readings),
                Arc::clone(&stop),
                render_node.to_owned(),
            );
            std::thread::spawn(move || {
                let mut next = Instant::now() + PERIOD;
                while !stop.load(Ordering::Relaxed) {
                    std::thread::sleep(Duration::from_millis(50));
                    if Instant::now() >= next {
                        let reading = Reading::take(&proc, &node);
                        readings
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .push(reading);
                        next += PERIOD;
                    }
                }
            })
        };
        MachineWatch {
            readings,
            stop,
            thread,
            render_node: render_node.to_owned(),
        }
    }

    /// Takes a last reading and gives the window's load.
    pub fn finish(self) -> Load {
        self.stop.store(true, Ordering::Relaxed);
        let _ = self.thread.join();
        let mut readings =
            std::mem::take(&mut *self.readings.lock().unwrap_or_else(|e| e.into_inner()));
        readings.push(Reading::take(&Proc::system(), &self.render_node));
        Load::of(&readings)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Writes `files`, each a path under `root` and its text.
    fn write_all(root: &Path, files: &[(&str, &str)]) {
        for (path, text) in files {
            let path = root.join(path);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, text).unwrap();
        }
    }

    #[test]
    fn watts_are_read_from_the_gpu_and_a_discharging_battery() {
        let sys = crate::testing::folder("machine-watts");
        // A GPU behind renderD128 at 12.5 W, and a laptop's battery giving 9.8 W.
        write_all(
            &sys,
            &[
                (
                    "devices/pci0000:00/0000:03:00.0/hwmon/hwmon2/power1_average",
                    "12500000\n",
                ),
                (
                    "devices/pci0000:00/0000:03:00.0/power_dpm_force_performance_level",
                    "auto\n",
                ),
                ("class/power_supply/BAT0/type", "Battery\n"),
                ("class/power_supply/BAT0/status", "Discharging\n"),
                ("class/power_supply/BAT0/power_now", "9800000\n"),
                ("class/power_supply/AC/type", "Mains\n"),
                // A wireless mouse's battery, discharging, is not the machine's.
                ("class/power_supply/hidpp_battery_0/type", "Battery\n"),
                ("class/power_supply/hidpp_battery_0/scope", "Device\n"),
                ("class/power_supply/hidpp_battery_0/status", "Discharging\n"),
                ("class/power_supply/hidpp_battery_0/power_now", "100000\n"),
            ],
        );
        fs::create_dir_all(sys.join("class/drm/renderD128")).unwrap();
        std::os::unix::fs::symlink(
            sys.join("devices/pci0000:00/0000:03:00.0"),
            sys.join("class/drm/renderD128/device"),
        )
        .unwrap();
        let state = MachineState::read_at(&sys, "/dev/dri/renderD128");
        assert_eq!(state.power_w, Some(12.5));
        assert_eq!(state.performance_level.as_deref(), Some("auto"));
        assert_eq!(state.on_battery, Some(true));
        assert_eq!(state.battery_w, Some(9.8));
        // The Steam Deck's battery gives its current and voltage alone: 1.5 A at 8 V.
        fs::remove_file(sys.join("class/power_supply/BAT0/power_now")).unwrap();
        write_all(
            &sys,
            &[
                ("class/power_supply/BAT0/current_now", "1500000\n"),
                ("class/power_supply/BAT0/voltage_now", "8000000\n"),
            ],
        );
        assert_eq!(battery(&sys), (Some(true), Some(12.0)));
        // A negative current, as some batteries report a draw, is the same draw.
        write_all(
            &sys,
            &[("class/power_supply/BAT0/current_now", "-1500000\n")],
        );
        assert_eq!(battery(&sys), (Some(true), Some(12.0)));
        // On mains the battery charges: its figures are not the machine's draw.
        write_all(&sys, &[("class/power_supply/BAT0/status", "Charging\n")]);
        assert_eq!(battery(&sys), (Some(false), None));
        // A battery whose figures cannot be read gives no watts, not 0.
        write_all(&sys, &[("class/power_supply/BAT0/status", "Discharging\n")]);
        fs::remove_file(sys.join("class/power_supply/BAT0/voltage_now")).unwrap();
        assert_eq!(battery(&sys), (Some(true), None));
        // A desktop has no battery at all: unknown, not mains.
        let desktop = crate::testing::folder("machine-no-battery");
        assert_eq!(battery(&desktop), (None, None));
        let state = MachineState::read_at(&desktop, "/dev/dri/renderD128");
        assert_eq!((state.power_w, state.battery_w), (None, None));
    }

    #[test]
    fn connected_screens_and_every_gpus_level_are_read() {
        let sys = crate::testing::folder("machine-screens");
        write_all(
            &sys,
            &[
                ("class/drm/card1-DP-1/status", "connected\n"),
                ("class/drm/card1-DP-1/enabled", "enabled\n"),
                ("class/drm/card1-DP-1/dpms", "On\n"),
                ("class/drm/card1-DP-1/modes", "3840x2160\n2560x1440\n"),
                ("class/drm/card1-HDMI-A-1/status", "disconnected\n"),
                (
                    "class/drm/card1/device/power_dpm_force_performance_level",
                    "high\n",
                ),
                (
                    "class/drm/card0/device/power_dpm_force_performance_level",
                    "auto\n",
                ),
                // An Intel GPU has no such file; a render node is no card.
                ("class/drm/card2/device/vendor", "0x8086\n"),
                (
                    "class/drm/renderD128/device/power_dpm_force_performance_level",
                    "auto\n",
                ),
            ],
        );
        let screens = screens(&sys);
        assert_eq!(screens.len(), 1);
        let screen = &screens["card1-DP-1"];
        assert_eq!(screen.preferred_mode.as_deref(), Some("3840x2160"));
        assert_eq!(screen.dpms.as_deref(), Some("On"));
        let levels = gpu_levels(&sys);
        assert_eq!(
            levels
                .iter()
                .map(|level| level.level.as_str())
                .collect::<Vec<_>>(),
            ["auto", "high"]
        );
        assert!(
            levels[1]
                .file
                .ends_with("card1/device/power_dpm_force_performance_level")
        );
    }

    #[test]
    fn the_current_clock_is_the_starred_line() {
        assert_eq!(
            parse_current_clock("0: 96Mhz\n1: 456Mhz\n3: 1000Mhz *\n"),
            Some(1000)
        );
        assert_eq!(
            parse_current_clock("S: 0Mhz *\n1: 500Mhz\n2: 2660Mhz\n"),
            Some(0)
        );
        assert_eq!(parse_current_clock("0: 96Mhz\n"), None);
    }

    #[test]
    fn the_machines_busy_time_leaves_idle_and_disk_waits_out() {
        let stat = "cpu  100 2 30 5000 40 5 6 7 0 0\ncpu0 50 1 15 2500 20 2 3 3 0 0\n";
        assert_eq!(parse_busy_ticks(stat), Some(100 + 2 + 30 + 5 + 6 + 7));
        assert_eq!(parse_busy_ticks("intr 1 2 3\n"), None);
    }

    #[test]
    fn descendants_are_found_in_any_order() {
        let processes = [(30, 20, 0), (20, 10, 0), (40, 1, 0), (10, 1, 0)];
        assert_eq!(descendants(10, &processes), BTreeSet::from([10, 20, 30]));
    }

    #[test]
    fn the_other_programs_load_leaves_the_benchs_own_out() {
        let at = Instant::now();
        let reading = |seconds: u64, busy: u64, own: u64, gpu_ns: Option<u64>| Reading {
            at: at + Duration::from_secs(seconds),
            busy_ticks: Some(busy),
            own_ticks: own,
            others_gpu_ns: gpu_ns
                .map(|ns| BTreeMap::from([(7, ns)]))
                .unwrap_or_default(),
            state: MachineState::default(),
        };
        // Over 4 s: 1,000 ticks busy, 600 of them the bench's; process 7 drew 0.2 s, all of it in
        // the second half.
        let load = Load::of(&[
            reading(0, 0, 0, Some(0)),
            reading(2, 300, 200, Some(0)),
            reading(4, 1_000, 600, Some(200_000_000)),
        ]);
        assert!((load.others_cpu_cores_mean.unwrap() - 1.0).abs() < 1e-9);
        assert!((load.others_cpu_cores_max.unwrap() - 1.5).abs() < 1e-9);
        assert!((load.bench_cpu_cores_mean - 1.5).abs() < 1e-9);
        assert!((load.others_gpu_percent_mean.unwrap() - 5.0).abs() < 1e-9);
        assert!((load.others_gpu_percent_max.unwrap() - 10.0).abs() < 1e-9);
        assert_eq!(load.disturbed, Some(false));
        assert_eq!(load.seconds.len(), 2);
        // Three cores of others: disturbed. No GPU file read: unknown, not nil.
        let busy = Load::of(&[reading(0, 0, 0, None), reading(1, 300, 0, None)]);
        assert_eq!(busy.disturbed, Some(true));
        assert_eq!(busy.others_gpu_percent_mean, None);
    }

    #[test]
    fn a_process_that_appears_counts_from_its_second_reading() {
        let at = Instant::now();
        let reading = |seconds: u64, gpu_ns: Option<u64>| Reading {
            at: at + Duration::from_secs(seconds),
            busy_ticks: Some(0),
            own_ticks: 0,
            others_gpu_ns: gpu_ns
                .map(|ns| BTreeMap::from([(7, ns)]))
                .unwrap_or_default(),
            state: MachineState::default(),
        };
        let load = Load::of(&[reading(0, None), reading(2, Some(5_000_000_000))]);
        assert_eq!(load.others_gpu_percent_mean, Some(0.0));
        assert_eq!(load.others_gpu_processes, 1);
    }

    #[test]
    fn only_the_drawing_engines_count_as_the_other_programs_gpu_time() {
        let root = crate::testing::folder("machine-drawing-engines");
        let process = root.join("100");
        fs::create_dir_all(process.join("fd")).unwrap();
        fs::create_dir_all(process.join("fdinfo")).unwrap();
        // A process of someone else's, parent 1, that draws 300 ns and decodes video for 9,000 ns.
        fs::write(
            process.join("stat"),
            "100 (player) S 1 100 100 0 -1 0 0 0 0 0 7 3 20 5 20 0 1 0 1000 1000000 100\n",
        )
        .unwrap();
        std::os::unix::fs::symlink("/dev/dri/renderD128", process.join("fd/7")).unwrap();
        fs::write(
            process.join("fdinfo/7"),
            "drm-client-id:\t1\ndrm-engine-gfx:\t200 ns\ndrm-engine-compute:\t100 ns\n\
             drm-engine-dec:\t9000 ns\n",
        )
        .unwrap();
        let reading = Reading::take(&Proc::at(&root), "/dev/dri/renderD128");
        assert_eq!(reading.others_gpu_ns, BTreeMap::from([(100, 300)]));
    }
}
