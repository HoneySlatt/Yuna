//! How each compositor is run: one recipe each, as data. A recipe gives the compositor's
//! program, its environment, the configuration file it writes into the run's folder, how the
//! bench tells it is ready, what it is told once ready (its output's mode, where the
//! configuration cannot say it), how the output it made is read back, its version (from its store
//! path, or Yuna's commit: never by running it), how it is stopped, what it is told to do in the
//! `motion` scenario, and how its own frames are counted.
//!
//! Each recipe has two configurations: *as shipped*, its defaults, and *equal work*, its effects,
//! wallpaper and XWayland off ([ADR-0072](../../../docs/adr/0072-budgets-set-against-the-peers-the-same-day.md)).
//! [`Plan::settings`] records what differs between them.
//!
//! Every rival of [ADR-0071](../../../docs/adr/0071-the-rivals-measured-headless-by-agents-and-on-a-real-display-by-the-owner.md)
//! has a recipe. Those that cannot run without a display say why instead ([`TOUR_ONLY`]): their
//! figures come from the owner's tour alone.
//!
//! Each recipe names its backend's pace ([`Pace`]): the rivals on wlroots' headless backend
//! (sway, dwl, mango, Umbriel) draw a frame 4 ms after each commit, not on a 240 Hz grid.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use crate::keys::Chord;
use crate::procfs;
use crate::sandbox::{Folders, Parent, RENDER_NODE, ShellPaths};

/// The output every compositor is given: 3840 × 2160 physical pixels at 240 Hz, scale 1.5.
pub const OUTPUT: &str = "3840x2160@240,1.5";
/// Its size in physical pixels, its refresh in mHz, and its scale.
pub const OUTPUT_WIDTH: u32 = 3840;
/// Its height.
pub const OUTPUT_HEIGHT: u32 = 2160;
/// Its refresh, in mHz.
pub const OUTPUT_MILLIHERTZ: u32 = 240_000;
/// Its scale.
pub const OUTPUT_SCALE: f64 = 1.5;

/// The most frames a second wlroots' headless backend draws for [`OUTPUT_MILLIHERTZ`]: it sends
/// a frame event `frame_delay = 1000000 / refresh` after each commit, in whole milliseconds
/// (`backend/headless/output.c:31` and `:76`, `int frame_delay; // ms` in
/// `include/backend/headless.h:24`, wlroots 0.20.2). 1000000 / 240000 is 4 ms: 250 frames a
/// second when frames take no time to draw, fewer as they take more, 241.7 to 243.9 in `game` on
/// 2026-11-16.
pub const WLROOTS_HEADLESS_HZ: f64 = 1000.0 / (1_000_000 / OUTPUT_MILLIHERTZ) as f64;

/// How a backend paces a compositor whose client draws as fast as the compositor takes its
/// frames, as `game`'s and `pacing`'s do: what such a scenario's costs a second scale with, and
/// so what they compare within.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Pace {
    /// wlroots' headless backend: a frame 4 ms after each commit, at most
    /// [`WLROOTS_HEADLESS_HZ`] frames a second, fewer under load.
    WlrootsTimer,
    /// At the mode's rate, 240 frames a second at most: KWin's and Yuna's grid of refreshes,
    /// Mutter's frame clock, Aquamarine's headless timer.
    ModeRate,
}

impl Pace {
    /// Its name in the summary.
    pub fn describe(self) -> &'static str {
        match self {
            Pace::WlrootsTimer => "on wlroots' headless timer, a frame 4 ms after each commit",
            Pace::ModeRate => "at the mode's rate, 240 frames a second at most",
        }
    }
}

/// Its size in logical pixels, which a fullscreen window is configured to: 2560 × 1440.
pub fn output_logical_size() -> (u32, u32) {
    (
        (f64::from(OUTPUT_WIDTH) / OUTPUT_SCALE).round() as u32,
        (f64::from(OUTPUT_HEIGHT) / OUTPUT_SCALE).round() as u32,
    )
}

/// A recipe's two configurations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Configuration {
    /// The compositor's defaults.
    AsShipped,
    /// Its effects, wallpaper and XWayland off.
    EqualWork,
}

impl Configuration {
    /// Both, in order.
    pub const ALL: [Configuration; 2] = [Configuration::AsShipped, Configuration::EqualWork];

    /// Its name on the command line and in the run's file.
    pub fn name(self) -> &'static str {
        match self {
            Configuration::AsShipped => "as-shipped",
            Configuration::EqualWork => "equal-work",
        }
    }

    /// The configuration named `name`.
    pub fn named(name: &str) -> Option<Configuration> {
        Configuration::ALL.into_iter().find(|c| c.name() == name)
    }
}

/// How a compositor's own frames are counted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Frames {
    /// By `ext-image-copy-capture`, a frame asked only on damage, in a pass apart from the
    /// costs': its copies cost the compositor.
    Capture,
    /// By KWin's `KWIN_LOG_PERFORMANCE_DATA` CSV, a row per frame presented, in the cost pass:
    /// exact, and cheap enough to keep on.
    KwinLog,
    /// By Umbriel's own count of its output's accepted commits, `commits` in `umbriel outputs
    /// --json`, read at the cost window's two ends: exact, in the cost pass.
    UmbrielCommits,
    /// Not counted, for the reason given: but by a client that counts its own, mpv in `game`.
    None(&'static str),
}

impl Frames {
    /// What the run's file says of it.
    pub fn describe(self) -> &'static str {
        match self {
            Frames::Capture => {
                "ext-image-copy-capture of the output, a frame asked only on damage, in a pass \
                 apart from the cost pass"
            }
            Frames::KwinLog => "KWin's KWIN_LOG_PERFORMANCE_DATA CSV, in the cost pass",
            Frames::UmbrielCommits => {
                "Umbriel's count of its output's commits, `umbriel outputs --json`, read at the \
                 cost window's ends"
            }
            Frames::None(why) => why,
        }
    }
}

/// How the bench tells a compositor is ready, beside its Wayland socket's appearing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ready {
    /// `swaymsg -t get_outputs` answers with the output active.
    Swaymsg,
    /// The name is owned on its session bus.
    BusName(&'static str),
    /// Its standard output holds a line that starts so.
    Line(&'static str),
    /// The action succeeds: its own tool answers.
    Answers(Action),
}

/// How the bench reads back the output a compositor made, which the run's file records.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Output {
    /// `swaymsg -t get_outputs`.
    Swaymsg,
    /// Yuna's log line `virtual output created`.
    YunaLog,
    /// KWin's frame log, once the case has ended: the refresh it presented at.
    KwinLog,
    /// `wlr-randr --json`, through wlr-output-management.
    WlrRandr,
    /// `umbriel outputs --json`.
    Umbriel,
    /// `hyprctl -j monitors all`.
    Hyprctl,
    /// `gdctl show`, through Mutter's `org.gnome.Mutter.DisplayConfig`.
    Gdctl,
}

/// A command sent to a compositor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// A sway command, through `swaymsg`.
    Sway(&'static str),
    /// A method of `org.kde.KWin` on `/KWin`, through `dbus-send`, with its arguments.
    KwinBus(&'static str, &'static [&'static str]),
    /// A key chord, typed into the compositor through the bench's virtual keyboard (`keys`),
    /// which its sandbox connects to that compositor alone.
    Keys(Chord),
    /// `mmsg <arguments>`, to mango's socket.
    Mmsg(&'static [&'static str]),
    /// `umbriel <arguments>`, to Umbriel's socket.
    Umbriel(&'static [&'static str]),
    /// `hyprctl <arguments>`, to the Hyprland of the case's runtime folder.
    Hyprctl(&'static [&'static str]),
    /// `wlr-randr <arguments>`, through wlr-output-management.
    WlrRandr(&'static [&'static str]),
}

impl Action {
    /// Its command, as a person would type it: `focus right`, `alt+j`, `dispatch view,2,0`.
    pub fn text(self) -> String {
        match self {
            Action::Sway(command) => command.to_owned(),
            Action::KwinBus(method, arguments) => format!("{method} {}", arguments.join(" ")),
            Action::Keys(chord) => chord.text(),
            Action::Mmsg(arguments)
            | Action::Umbriel(arguments)
            | Action::Hyprctl(arguments)
            | Action::WlrRandr(arguments) => arguments.join(" "),
        }
    }
}

/// One compositor's recipe.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Recipe {
    /// Its name on the command line.
    pub name: &'static str,
    /// The backend it runs on headless.
    pub backend: &'static str,
    /// How that backend paces its frames.
    pub pace: Pace,
    /// What it draws with.
    pub renderer: &'static str,
    /// How its frames are counted.
    pub frames: Frames,
    /// How it tells it is ready.
    pub ready: Ready,
    /// What it is told once ready, in order, before anything is measured.
    pub setup: &'static [Action],
    /// How the output it made is read back.
    pub output: Output,
    /// The command that stops it; without one, SIGTERM.
    pub stop: Option<Action>,
    /// The `motion` scenario's actions, each run once the last settled; empty if it takes none.
    pub actions: &'static [Action],
    /// Why it has no actions, when it has none.
    pub no_actions: Option<&'static str>,
    /// A line its log holds when it was restarted during the case, which fails the case: the
    /// compositor measured is no longer the one started.
    pub restarted: Option<&'static str>,
}

const SWAY_ACTIONS: &[Action] = &[
    Action::Sway("focus right"),
    Action::Sway("focus right"),
    Action::Sway("focus right"),
    Action::Sway("focus left"),
    Action::Sway("focus left"),
    Action::Sway("focus left"),
    Action::Sway("workspace 2"),
    Action::Sway("workspace 1"),
];

/// A chord of Alt and `key`, dwl's modifier.
const fn alt(key: &'static str) -> Action {
    Action::Keys(Chord {
        modifiers: &["alt"],
        key,
    })
}

/// dwl's default bindings (`config.def.h`): Alt+J and Alt+K focus the next and previous window
/// of the stack, Alt+2 and Alt+1 view another tag and back.
const DWL_ACTIONS: &[Action] = &[
    alt("j"),
    alt("j"),
    alt("j"),
    alt("k"),
    alt("k"),
    alt("k"),
    alt("2"),
    alt("1"),
];

const MANGO_ACTIONS: &[Action] = &[
    Action::Mmsg(&["dispatch", "focusstack,next"]),
    Action::Mmsg(&["dispatch", "focusstack,next"]),
    Action::Mmsg(&["dispatch", "focusstack,next"]),
    Action::Mmsg(&["dispatch", "focusstack,prev"]),
    Action::Mmsg(&["dispatch", "focusstack,prev"]),
    Action::Mmsg(&["dispatch", "focusstack,prev"]),
    Action::Mmsg(&["dispatch", "view,2,0"]),
    Action::Mmsg(&["dispatch", "view,1,0"]),
];

const UMBRIEL_ACTIONS: &[Action] = &[
    Action::Umbriel(&["msg", "window-focus-next"]),
    Action::Umbriel(&["msg", "window-focus-next"]),
    Action::Umbriel(&["msg", "window-focus-next"]),
    Action::Umbriel(&["msg", "window-focus-previous"]),
    Action::Umbriel(&["msg", "window-focus-previous"]),
    Action::Umbriel(&["msg", "window-focus-previous"]),
    Action::Umbriel(&["msg", "workspace-switch", "2"]),
    Action::Umbriel(&["msg", "workspace-switch", "1"]),
];

const HYPRLAND_ACTIONS: &[Action] = &[
    Action::Hyprctl(&["dispatch", "hl.dsp.window.cycle_next()"]),
    Action::Hyprctl(&["dispatch", "hl.dsp.window.cycle_next()"]),
    Action::Hyprctl(&["dispatch", "hl.dsp.window.cycle_next()"]),
    Action::Hyprctl(&["dispatch", "hl.dsp.window.cycle_next({ next = false })"]),
    Action::Hyprctl(&["dispatch", "hl.dsp.window.cycle_next({ next = false })"]),
    Action::Hyprctl(&["dispatch", "hl.dsp.window.cycle_next({ next = false })"]),
    Action::Hyprctl(&["dispatch", "hl.dsp.focus({ workspace = 2 })"]),
    Action::Hyprctl(&["dispatch", "hl.dsp.focus({ workspace = 1 })"]),
];

/// The output Hyprland is told to make on its headless backend, and which its monitor rule sets.
const HYPRLAND_OUTPUT: &str = "HEADLESS-1";

/// The recipes, by name.
pub const RECIPES: [Recipe; 9] = [
    Recipe {
        name: "sway",
        backend: "wlroots-headless",
        pace: Pace::WlrootsTimer,
        renderer: "gles2",
        frames: Frames::Capture,
        ready: Ready::Swaymsg,
        setup: &[],
        output: Output::Swaymsg,
        stop: Some(Action::Sway("exit")),
        actions: SWAY_ACTIONS,
        no_actions: None,
        restarted: None,
    },
    Recipe {
        name: "sway-vulkan",
        backend: "wlroots-headless",
        pace: Pace::WlrootsTimer,
        renderer: "vulkan",
        frames: Frames::Capture,
        ready: Ready::Swaymsg,
        setup: &[],
        output: Output::Swaymsg,
        stop: Some(Action::Sway("exit")),
        actions: SWAY_ACTIONS,
        no_actions: None,
        restarted: None,
    },
    Recipe {
        name: "kwin",
        backend: "kwin-virtual",
        pace: Pace::ModeRate,
        renderer: "gles (KWin's virtual backend draws with OpenGL ES alone)",
        frames: Frames::KwinLog,
        ready: Ready::BusName("org.kde.KWin"),
        setup: &[],
        output: Output::KwinLog,
        stop: None,
        actions: &[
            Action::KwinBus("setCurrentDesktop", &["int32:2"]),
            Action::KwinBus("setCurrentDesktop", &["int32:1"]),
        ],
        no_actions: None,
        restarted: None,
    },
    Recipe {
        name: "yuna",
        backend: "yuna-headless",
        pace: Pace::ModeRate,
        renderer: "gles2 (Smithay's GLES renderer on the render node, --gpu)",
        // Its `frame drawn` lines need its debug logging, which would weigh on its costs.
        frames: Frames::Capture,
        ready: Ready::Line("WAYLAND_DISPLAY="),
        setup: &[],
        output: Output::YunaLog,
        stop: None,
        actions: &[],
        no_actions: Some(
            "`yuna --headless` takes no command from outside; MCP's layout tools do not exist \
             (ADR-0056)",
        ),
        restarted: None,
    },
    Recipe {
        name: "dwl",
        backend: "wlroots-headless",
        pace: Pace::WlrootsTimer,
        renderer: "gles2",
        frames: Frames::Capture,
        // Ready once it answers wlr-output-management; its output's mode is read back after the
        // setup.
        ready: Ready::Answers(Action::WlrRandr(&[])),
        // dwl gives a new output its preferred mode, which a headless output has none of
        // (`dwl.c:1178`), and reads no configuration file: wlr-randr sets the bench's.
        setup: &[Action::WlrRandr(&[
            "--output",
            "HEADLESS-1",
            "--custom-mode",
            "3840x2160@240Hz",
            "--scale",
            "1.5",
        ])],
        output: Output::WlrRandr,
        stop: None,
        actions: DWL_ACTIONS,
        no_actions: None,
        restarted: None,
    },
    Recipe {
        name: "mango",
        backend: "wlroots-headless",
        pace: Pace::WlrootsTimer,
        renderer: "gles2 (SceneFX's fx_renderer)",
        frames: Frames::Capture,
        ready: Ready::Answers(Action::Mmsg(&["get", "all-monitors"])),
        setup: &[],
        output: Output::WlrRandr,
        stop: None,
        actions: MANGO_ACTIONS,
        no_actions: None,
        restarted: None,
    },
    Recipe {
        name: "umbriel",
        backend: "wlroots-headless",
        pace: Pace::WlrootsTimer,
        renderer: "gles2 (umbrielfx, Umbriel's fork of SceneFX)",
        frames: Frames::UmbrielCommits,
        ready: Ready::Answers(Action::Umbriel(&["outputs"])),
        setup: &[],
        output: Output::Umbriel,
        stop: None,
        actions: UMBRIEL_ACTIONS,
        no_actions: None,
        restarted: None,
    },
    Recipe {
        name: "hyprland",
        backend: "aquamarine-headless",
        pace: Pace::ModeRate,
        renderer: "gles (Hyprland's OpenGL renderer, on its parent's render node)",
        frames: Frames::Capture,
        ready: Ready::Answers(Action::Hyprctl(&["monitors"])),
        // Its headless backend makes no output by itself; the monitor rule gives this one its
        // mode, and disables the window on its parent.
        setup: &[Action::Hyprctl(&[
            "output",
            "create",
            "headless",
            HYPRLAND_OUTPUT,
        ])],
        output: Output::Hyprctl,
        stop: None,
        actions: HYPRLAND_ACTIONS,
        no_actions: None,
        // start-hyprland restarts a Hyprland that ended badly, in its safe mode
        // (start/src/main.cpp:114).
        restarted: Some("Hyprland exit not-cleanly, restarting"),
    },
    Recipe {
        name: "mutter",
        backend: "mutter-headless",
        pace: Pace::ModeRate,
        renderer: "gles (Cogl's GL driver on the render node's GBM device)",
        frames: Frames::None(
            "not counted: Mutter offers no ext-image-copy-capture for the bench's counter, and \
             CLUTTER_SHOW_FPS prints a rate each second, which a window's ends do not cut",
        ),
        ready: Ready::BusName("org.gnome.Mutter.DisplayConfig"),
        setup: &[],
        output: Output::Gdctl,
        stop: None,
        actions: &[],
        no_actions: Some(
            "Mutter takes no command for its windows from outside: GNOME Shell drives them from \
             inside Mutter's own process, and is not measured at M1.9",
        ),
        restarted: None,
    },
];

/// The recipe named `name`.
pub fn recipe(name: &str) -> Option<&'static Recipe> {
    RECIPES.iter().find(|recipe| recipe.name == name)
}

/// The rivals ADR-0071 names, by their recipes' names.
pub const ADR_0071_RIVALS: [&str; 9] = [
    "dwl",
    "sway",
    "hyprland",
    "niri",
    "mango",
    "umbriel",
    "kwin",
    "mutter",
    "cosmic-comp",
];

/// The recipe of a rival that cannot run without a display: its figures come from the owner's
/// tour alone (ADR-0071).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TourOnly {
    /// Its name on the command line.
    pub name: &'static str,
    /// Why it cannot run headless.
    pub why: &'static str,
}

/// The rivals that cannot run headless, from their sources (docs/survey.md).
pub const TOUR_ONLY: [TourOnly; 2] = [
    TourOnly {
        name: "niri",
        why: "niri's headless backend serves its own tests only, in their process \
              (src/backend/headless.rs:1, `#[cfg(test)]` in src/lib.rs:28); its program has no \
              headless mode (src/main.rs:182 passes headless = false)",
    },
    TourOnly {
        name: "cosmic-comp",
        why: "cosmic-comp has no headless backend: KMS, X11 or winit alone \
              (src/backend/mod.rs:25-43), the first on the owner's display, the others in a \
              window of another session",
    },
];

/// The tour-only rival named `name`.
pub fn tour_only(name: &str) -> Option<&'static TourOnly> {
    TOUR_ONLY.iter().find(|rival| rival.name == name)
}

/// What the bench made before the cases: Yuna's program and commit, and KWin's output
/// configuration at 240 Hz.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Prepared {
    /// `yuna`, built for release from the workspace.
    pub yuna: Option<PathBuf>,
    /// The workspace's commit, `-dirty` if its tree differed.
    pub yuna_version: Option<String>,
    /// KWin's `kwinoutputconfig.json`, edited to 240 Hz; none if it could not be made.
    pub kwin_output_config: Option<String>,
}

/// A compositor's start, as a recipe and a configuration make it.
#[derive(Debug, Clone, PartialEq)]
pub struct Plan {
    /// The program.
    pub program: PathBuf,
    /// Its arguments.
    pub arguments: Vec<String>,
    /// Its `PATH`.
    pub path: String,
    /// Its own variables, beside the sandbox's.
    pub variables: Vec<(String, String)>,
    /// Its version.
    pub version: Option<String>,
    /// What differs between its two configurations, as this one sets it.
    pub settings: BTreeMap<String, String>,
    /// The files it reads, written into its folders: path and text.
    pub files: Vec<(PathBuf, String)>,
    /// The compositor it runs inside, in the same sandbox, if it needs one.
    pub parent: Option<Parent>,
    /// The short link to its runtime folder it is given as `XDG_RUNTIME_DIR`, if it needs one
    /// ([`Folders::short_runtime`]).
    pub runtime_link: Option<PathBuf>,
}

fn bin(package: &Path) -> String {
    package.join("bin").to_string_lossy().into_owned()
}

fn settings(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
        .collect()
}

fn version(package: &Path) -> Option<String> {
    procfs::store_version(&package.to_string_lossy()).map(str::to_owned)
}

fn words(words: &[&str]) -> Vec<String> {
    words.iter().map(|word| (*word).to_owned()).collect()
}

/// The variables that put a wlroots compositor on its headless backend, one output, drawing on
/// the render node; with `renderer`, the renderer wlroots creates.
fn wlroots_headless(renderer: Option<&str>) -> Vec<(String, String)> {
    let mut variables = vec![
        ("WLR_BACKENDS".to_owned(), "headless".to_owned()),
        ("WLR_HEADLESS_OUTPUTS".to_owned(), "1".to_owned()),
        ("WLR_RENDER_DRM_DEVICE".to_owned(), RENDER_NODE.to_owned()),
    ];
    if let Some(renderer) = renderer {
        variables.push(("WLR_RENDERER".to_owned(), renderer.to_owned()));
    }
    variables
}

/// The plan of `recipe` in `configuration`, its files in `folders`.
pub fn plan(
    recipe: &Recipe,
    configuration: Configuration,
    shell: &ShellPaths,
    folders: &Folders,
    prepared: &Prepared,
) -> Result<Plan, String> {
    match recipe.name {
        "sway" | "sway-vulkan" => sway(recipe, configuration, shell, folders),
        "kwin" => kwin(configuration, shell, folders, prepared),
        "yuna" => yuna(shell, prepared),
        "dwl" => dwl(configuration, shell),
        "mango" => mango(configuration, shell, folders),
        "umbriel" => umbriel(configuration, shell, folders),
        "hyprland" => hyprland(configuration, shell, folders),
        "mutter" => mutter(configuration, shell, folders),
        name => Err(format!("no plan for {name}")),
    }
}

/// sway on wlroots' headless backend, on the render node, with nixpkgs' shipped configuration.
fn sway(
    recipe: &Recipe,
    configuration: Configuration,
    shell: &ShellPaths,
    folders: &Folders,
) -> Result<Plan, String> {
    let shipped_path = shell.sway.join("etc/sway/config");
    let shipped = fs::read_to_string(&shipped_path)
        .map_err(|e| format!("{}: {e}", shipped_path.display()))?;
    let output =
        format!("output HEADLESS-1 mode --custom {OUTPUT_WIDTH}x{OUTPUT_HEIGHT}@240Hz scale 1.5\n");
    let (text, settings) = match configuration {
        Configuration::AsShipped => (
            format!("{shipped}\n{output}"),
            settings(&[
                ("bar", "swaybar, as shipped: its clock redrawn each second"),
                (
                    "xwayland",
                    "lazy, as shipped: Xwayland starts at the first X11 client",
                ),
                (
                    "wallpaper",
                    "none: nixpkgs' shipped configuration has its line commented out; sway \
                     still starts swaybg, which then draws nothing",
                ),
            ]),
        ),
        Configuration::EqualWork => (
            format!(
                "{}\nxwayland disable\nswaybg_command -\n{output}",
                without_block(&shipped, "bar {")
            ),
            settings(&[
                ("bar", "removed"),
                ("xwayland", "disable"),
                ("wallpaper", "none, and no swaybg: swaybg_command -"),
            ]),
        ),
    };
    let config = folders.config().join("sway/config");
    Ok(Plan {
        program: shell.sway.join("bin/sway"),
        arguments: vec!["--config".to_owned(), config.to_string_lossy().into_owned()],
        // swaybar runs its status command in `sh`, with `date` and `sleep`.
        path: [bin(&shell.sway), bin(&shell.coreutils), bin(&shell.bash)].join(":"),
        variables: wlroots_headless(Some(recipe.renderer)),
        version: version(&shell.sway),
        settings,
        files: vec![(config, text)],
        parent: None,
        runtime_link: None,
    })
}

/// `text` without the block that starts at the line beginning with `opening` and ends at its
/// matching `}`.
pub fn without_block(text: &str, opening: &str) -> String {
    let mut kept = String::with_capacity(text.len());
    let mut depth = 0usize;
    for line in text.lines() {
        if depth == 0 && line.trim_start().starts_with(opening) {
            depth = 1;
            continue;
        }
        if depth > 0 {
            let code = line.split('#').next().unwrap_or("");
            depth += code.matches('{').count();
            depth = depth.saturating_sub(code.matches('}').count());
            continue;
        }
        kept.push_str(line);
        kept.push('\n');
    }
    kept
}

/// KWin's effects on by default (`"EnabledByDefault": true` in KWin 6.8.2's metadata), which
/// *equal work* turns off in `kwinrc`'s `[Plugins]`.
pub const KWIN_DEFAULT_EFFECTS: [&str; 30] = [
    "blendchanges",
    "blur",
    "colorpicker",
    "dialogparent",
    "dimscreen",
    "fadingpopups",
    "frozenapp",
    "fullscreen",
    "highlightwindow",
    "kscreen",
    "login",
    "logout",
    "maximize",
    "outputlocator",
    "overview",
    "scale",
    "screenedge",
    "screentransform",
    "sessionquit",
    "shakecursor",
    "slide",
    "slidingnotifications",
    "slidingpopups",
    "squash",
    "startupfeedback",
    "systembell",
    "tileseditor",
    "windowaperture",
    "windowview",
    "zoom",
];

/// KWin's virtual backend, at 2560 × 1440 logical pixels at scale 1.5, 3840 × 2160 physical,
/// on the render node; its 240 Hz from the output configuration [`Prepared`] holds.
fn kwin(
    configuration: Configuration,
    shell: &ShellPaths,
    folders: &Folders,
    prepared: &Prepared,
) -> Result<Plan, String> {
    let mut arguments = words(&[
        "--virtual",
        "--width",
        "2560",
        "--height",
        "1440",
        "--scale",
        "1.5",
        "--no-lockscreen",
        "--no-global-shortcuts",
        "--no-kactivities",
    ]);
    // Two desktops, for the motion scenario's switch; the same in both configurations.
    let mut kwinrc = String::from("[Desktops]\nNumber=2\nRows=1\n");
    let settings = match configuration {
        Configuration::AsShipped => {
            // Plasma's session starts KWin so; its Xwayland then starts at the first X11 client.
            arguments.push("--xwayland".to_owned());
            settings(&[
                ("effects", "KWin's defaults"),
                ("xwayland", "--xwayland, as Plasma starts KWin: on demand"),
                ("wallpaper", "none: plasmashell draws it, and is not run"),
            ])
        }
        Configuration::EqualWork => {
            kwinrc.push_str("\n[Plugins]\n");
            for effect in KWIN_DEFAULT_EFFECTS {
                kwinrc.push_str(&format!("{effect}Enabled=false\n"));
            }
            settings(&[
                (
                    "effects",
                    "every effect on by default turned off in kwinrc [Plugins]",
                ),
                ("xwayland", "off: no --xwayland"),
                ("wallpaper", "none"),
            ])
        }
    };
    let mut files = vec![(folders.config().join("kwinrc"), kwinrc)];
    if let Some(output) = &prepared.kwin_output_config {
        files.push((
            folders.config().join("kwinoutputconfig.json"),
            output.clone(),
        ));
    }
    Ok(Plan {
        program: shell.kwin.join("bin/kwin_wayland"),
        arguments,
        path: [
            bin(&shell.kwin),
            bin(&shell.xwayland),
            bin(&shell.coreutils),
        ]
        .join(":"),
        variables: vec![
            // The render device KWin draws on; never a card node.
            ("KWIN_DRM_DEVICES".to_owned(), RENDER_NODE.to_owned()),
            ("KWIN_LOG_PERFORMANCE_DATA".to_owned(), "1".to_owned()),
        ],
        version: version(&shell.kwin),
        settings,
        files,
        parent: None,
        runtime_link: None,
    })
}

/// KWin's output configuration `written`, by KWin itself at its first start, with its first
/// output's mode made a custom 3840 × 2160 at 240 Hz.
pub fn kwin_output_at_240(written: &str) -> Result<String, String> {
    let mut document: serde_json::Value =
        serde_json::from_str(written).map_err(|e| format!("kwinoutputconfig.json: {e}"))?;
    let outputs = document
        .as_array_mut()
        .and_then(|sections| {
            sections
                .iter_mut()
                .find(|section| section["name"] == "outputs")
        })
        .and_then(|section| section["data"].as_array_mut())
        .and_then(|outputs| outputs.first_mut())
        .ok_or("kwinoutputconfig.json holds no output")?;
    // Generated | Custom: the flags KWin gives a custom mode, which it compares.
    let mode = serde_json::json!({
        "flags": 10,
        "basic": {
            "width": OUTPUT_WIDTH,
            "height": OUTPUT_HEIGHT,
            "refreshRate": OUTPUT_MILLIHERTZ,
        },
    });
    outputs["mode"] = mode.clone();
    outputs["customModes"] = serde_json::json!([mode]);
    outputs["scale"] = serde_json::json!(1.5);
    serde_json::to_string_pretty(&document).map_err(|e| e.to_string())
}

/// Yuna, built by the bench from the workspace, headless on the render node.
fn yuna(shell: &ShellPaths, prepared: &Prepared) -> Result<Plan, String> {
    let program = prepared.yuna.clone().ok_or("yuna was not built")?;
    let both = "the same in both configurations";
    Ok(Plan {
        program,
        arguments: words(&["--headless", "--gpu", "--output", OUTPUT]),
        // Yuna finds Xwayland on its PATH (ADR-0067). Its log stays at its default level.
        path: bin(&shell.xwayland),
        variables: Vec::new(),
        version: prepared.yuna_version.clone(),
        settings: settings(&[
            (
                "effects",
                &format!("none: animations are off headless by default (ADR-0055); {both}"),
            ),
            (
                "xwayland",
                &format!("on demand at the first X11 client (ADR-0067), no switch; {both}"),
            ),
            ("wallpaper", &format!("none; {both}")),
        ]),
        files: Vec::new(),
        parent: None,
        runtime_link: None,
    })
}

/// dwl on wlroots' headless backend, on the render node. It reads no configuration file: its
/// output's mode comes from wlr-randr, once it is ready (the recipe's `setup`).
fn dwl(configuration: Configuration, shell: &ShellPaths) -> Result<Plan, String> {
    let both = "the same in both configurations";
    let (package, xwayland) = match configuration {
        Configuration::AsShipped => (
            &shell.dwl,
            "lazy, as nixpkgs builds dwl: Xwayland starts at the first X11 client".to_owned(),
        ),
        Configuration::EqualWork => (
            &shell.dwl_no_xwayland,
            "off: dwl built without it (nixpkgs' enableXWayland = false); it has no switch at \
             run time"
                .to_owned(),
        ),
    };
    Ok(Plan {
        program: package.join("bin/dwl"),
        arguments: Vec::new(),
        path: [bin(package), bin(&shell.xwayland)].join(":"),
        variables: wlroots_headless(Some("gles2")),
        version: version(package),
        settings: settings(&[
            (
                "effects",
                &format!("none: dwl draws no effect and animates nothing; {both}"),
            ),
            ("xwayland", &xwayland),
            ("wallpaper", &format!("none; {both}")),
            (
                "bar",
                &format!("none: dwl writes its status to its standard output, its log; {both}"),
            ),
        ]),
        files: Vec::new(),
        parent: None,
        runtime_link: None,
    })
}

/// mango on wlroots' headless backend, on the render node, its shipped `config.conf` and a
/// `monitor_rule` for the bench's output.
fn mango(
    configuration: Configuration,
    shell: &ShellPaths,
    folders: &Folders,
) -> Result<Plan, String> {
    let shipped_path = shell.mango.join("etc/mango/config.conf");
    let shipped = fs::read_to_string(&shipped_path)
        .map_err(|e| format!("{}: {e}", shipped_path.display()))?;
    let rule = format!(
        "monitor_rule=name:HEADLESS-1,width:{OUTPUT_WIDTH},height:{OUTPUT_HEIGHT},refresh:240,\
         scale:1.5\n"
    );
    let (package, text, settings) = match configuration {
        Configuration::AsShipped => (
            &shell.mango,
            format!("{shipped}\n{rule}"),
            settings(&[
                (
                    "effects",
                    "its shipped config.conf: animations on, corners rounded at 6 px, blur and \
                     shadows off",
                ),
                (
                    "xwayland",
                    "started with mango, as shipped (xwayland_persistence = 1)",
                ),
                ("wallpaper", "none: a plain root colour"),
            ]),
        ),
        // Later lines win over the shipped ones.
        Configuration::EqualWork => (
            &shell.mango_no_xwayland,
            format!(
                "{shipped}\n{rule}animations=0\nlayer_animations=0\nborder_radius=0\nblur=0\n\
                 shadows=0\n"
            ),
            settings(&[
                (
                    "effects",
                    "animations, layer animations and rounded corners off (animations=0, \
                     layer_animations=0, border_radius=0), blur and shadows off as shipped",
                ),
                (
                    "xwayland",
                    "off: mango built without it (nixpkgs' enableXWayland = false); it has no \
                     switch at run time",
                ),
                ("wallpaper", "none: a plain root colour"),
            ]),
        ),
    };
    let config = folders.config().join("mango/config.conf");
    Ok(Plan {
        program: package.join("bin/mango"),
        arguments: vec!["-c".to_owned(), config.to_string_lossy().into_owned()],
        path: [bin(package), bin(&shell.xwayland)].join(":"),
        // SceneFX picks its render node as wlroots does; it reads no WLR_RENDERER.
        variables: wlroots_headless(None),
        version: version(package),
        settings,
        files: vec![(config, text)],
        parent: None,
        runtime_link: None,
    })
}

/// Umbriel on wlroots' headless backend, on the render node: its packaged configuration,
/// included, and the bench's output section after it.
fn umbriel(
    configuration: Configuration,
    shell: &ShellPaths,
    folders: &Folders,
) -> Result<Plan, String> {
    let shipped = shell.umbriel.join("share/umbriel/config.toml");
    if !shipped.exists() {
        return Err(format!("{}: missing", shipped.display()));
    }
    // An included file applies first, the including one last (docs/user/configuration.md).
    let mut text = format!(
        "[include]\nfiles = [\"{}\"]\n\n[output.HEADLESS-1]\nmode = \"{OUTPUT_WIDTH}x{OUTPUT_HEIGHT}@240\"\n\
         scale = 1.5\n",
        shipped.display()
    );
    let settings = match configuration {
        Configuration::AsShipped => settings(&[
            (
                "effects",
                "its packaged config.toml: animations on, blur behind every window (a window \
                 rule, drawn where a window is translucent), shadows on, corners rounded at \
                 10 px",
            ),
            (
                "cheatsheet",
                "shown at start, as shipped, until a key dismisses it: on screen throughout",
            ),
            (
                "xwayland",
                "lazy, as shipped: Xwayland starts at the first X11 client",
            ),
            ("wallpaper", "none: Umbriel draws none"),
        ]),
        Configuration::EqualWork => {
            text.push_str(
                "\n[general]\nxwayland = false\nshow_cheatsheet = false\n\n[appearance]\n\
                 corner_radius = 0\n\n[appearance.blur]\nenabled = false\n\n\
                 [appearance.shadow]\nenabled = false\n\n[animation]\nenabled = false\n",
            );
            settings(&[
                (
                    "effects",
                    "animations, blur, shadows and rounded corners off ([animation] enabled, \
                     [appearance.blur] and [appearance.shadow] enabled = false, corner_radius = \
                     0)",
                ),
                ("cheatsheet", "off: show_cheatsheet = false"),
                ("xwayland", "off: xwayland = false"),
                ("wallpaper", "none: Umbriel draws none"),
            ])
        }
    };
    let config = folders.config().join("umbriel/config.toml");
    Ok(Plan {
        // nixpkgs' wrapper, which puts Xwayland on its PATH.
        program: shell.umbriel.join("bin/umbriel"),
        arguments: vec!["-c".to_owned(), config.to_string_lossy().into_owned()],
        path: bin(&shell.umbriel),
        // umbrielfx creates GLES2 alone, and reads no WLR_RENDERER (docs/survey.md).
        variables: wlroots_headless(None),
        version: version(&shell.umbriel),
        settings,
        files: vec![(config, text)],
        parent: None,
        runtime_link: None,
    })
}

/// Hyprland, inside a headless dwl on the render node, its parent, which lends it the render node
/// its own headless backend cannot allocate without (Aquamarine's `Headless.cpp:261`). It draws
/// on its own headless output, which the bench tells it to make, on its own timer: the window it
/// opens on its parent is disabled.
///
/// The parent is dwl, not sway: Aquamarine's Wayland backend binds `xdg_wm_base` at version 6
/// (`src/backend/Wayland.cpp:125`), which sway 1.12 offers at 5 (`sway/server.c:77`) and dwl at 6
/// (`dwl.c:2718`), with `wl_compositor` 6 and wlroots' `wl_seat` 9, which it binds too.
fn hyprland(
    configuration: Configuration,
    shell: &ShellPaths,
    folders: &Folders,
) -> Result<Plan, String> {
    let shipped_path = shell.hyprland.join("share/hypr/hyprland.lua");
    let shipped = fs::read_to_string(&shipped_path)
        .map_err(|e| format!("{}: {e}", shipped_path.display()))?;
    let output = format!(
        "\n-- The bench's output, which `hyprctl output create headless` makes, and the window on\n\
         -- its parent, disabled: Hyprland draws on its own headless timer.\n\
         hl.monitor({{ output = \"{HYPRLAND_OUTPUT}\", mode = \"{OUTPUT_WIDTH}x{OUTPUT_HEIGHT}@240\", \
         position = \"0x0\", scale = \"1.5\" }})\n\
         hl.monitor({{ output = \"WAYLAND-1\", disabled = true }})\n"
    );
    let (text, path, settings) = match configuration {
        Configuration::AsShipped => (
            format!("{shipped}{output}"),
            // Its XWayland starts at launch when Xwayland is on its PATH (src/xwayland).
            [
                bin(&shell.hyprland),
                bin(&shell.coreutils),
                bin(&shell.xwayland),
            ]
            .join(":"),
            settings(&[
                (
                    "effects",
                    "its shipped hyprland.lua: animations, blur, shadows, corners rounded at 10 px",
                ),
                (
                    "wallpaper",
                    "its own default wallpaper (misc force_default_wallpaper = -1)",
                ),
                (
                    "xwayland",
                    "started with Hyprland, as shipped: it has no lazy mode (src/xwayland/Server.cpp)",
                ),
            ]),
        ),
        Configuration::EqualWork => (
            format!(
                "{shipped}{output}\nhl.config({{\n    decoration = {{ rounding = 0, shadow = {{ \
                 enabled = false }}, blur = {{ enabled = false }} }},\n    animations = {{ \
                 enabled = false }},\n    misc = {{ force_default_wallpaper = 0, \
                 disable_hyprland_logo = true }},\n    xwayland = {{ enabled = false }},\n}})\n"
            ),
            // It reads its system's description with `cat` at its start.
            [bin(&shell.hyprland), bin(&shell.coreutils)].join(":"),
            settings(&[
                (
                    "effects",
                    "animations, blur, shadows and rounded corners off in its configuration",
                ),
                (
                    "wallpaper",
                    "none: force_default_wallpaper = 0, disable_hyprland_logo = true",
                ),
                ("xwayland", "off: xwayland.enabled = false"),
            ]),
        ),
    };
    let (realtime, mut variables) = match configuration {
        Configuration::AsShipped => (
            "SCHED_RR on its main thread, as 0.57.1 ships: from rtkit, over the system bus \
             (src/init/initHelpers.cpp); RLIMIT_RTPRIO is 0, so the kernel alone would refuse it",
            Vec::new(),
        ),
        // rtkit is its system bus's only use in Hyprland and Aquamarine.
        Configuration::EqualWork => (
            "none, as 0.57.2 ships: its system bus is a socket that does not exist, so rtkit \
             cannot be asked, and RLIMIT_RTPRIO is 0",
            vec![(
                "DBUS_SYSTEM_BUS_ADDRESS".to_owned(),
                format!(
                    "unix:path={}",
                    folders.runtime().join("no-system-bus").display()
                ),
            )],
        ),
    };
    let config = folders.config().join("hypr/hyprland.lua");
    let parent_runtime = folders.runtime().join("parent");
    let mut parent_variables = wlroots_headless(Some("gles2"));
    parent_variables.push(("PATH".to_owned(), bin(&shell.dwl_no_xwayland)));
    variables.extend([
        ("XDG_CURRENT_DESKTOP".to_owned(), "Hyprland".to_owned()),
        // Aquamarine always tries a DRM session first (Backend.cpp's CDRMBackend::attempt);
        // libseat's logind backend would look for the user's display session. A backend name
        // libseat does not know opens no seat at all (libseat/libseat.c:45-55).
        ("LIBSEAT_BACKEND".to_owned(), "none".to_owned()),
        // Its instance's sockets, `$XDG_RUNTIME_DIR/hypr/<62 characters>/.socket.sock`, would
        // pass Unix's 108 bytes under the run's folder.
        (
            "XDG_RUNTIME_DIR".to_owned(),
            folders.short_runtime.to_string_lossy().into_owned(),
        ),
    ]);
    let mut settings = settings;
    settings.insert("realtime".to_owned(), realtime.to_owned());
    settings.insert(
        "parent".to_owned(),
        format!(
            "dwl {} built without XWayland, headless on the render node; Hyprland's window on \
             it, WAYLAND-1, is disabled; counted in the stack, not the core; the same in both \
             configurations. Not sway: Aquamarine binds xdg_wm_base 6, which sway 1.12 offers \
             at 5",
            version(&shell.dwl_no_xwayland).unwrap_or_default()
        ),
    );
    Ok(Plan {
        // As its session file starts it (share/wayland-sessions/hyprland.desktop): through its
        // watchdog, which runs `Hyprland` from the PATH, nixpkgs' wrapper. Started without it,
        // or without `XDG_CURRENT_DESKTOP=Hyprland`, which a display manager sets from the file's
        // DesktopNames, it shows a warning for 15 s each (src/Compositor.cpp:906-930). The
        // wrapper adds only store paths to its PATH: hyprland-dialog among them, which turns its
        // watch of hung clients on.
        program: shell.hyprland.join("bin/start-hyprland"),
        arguments: vec![
            "--".to_owned(),
            "--config".to_owned(),
            config.to_string_lossy().into_owned(),
        ],
        path,
        variables,
        version: version(&shell.hyprland),
        settings,
        files: vec![(config, text)],
        parent: Some(Parent {
            command: vec![
                shell
                    .dwl_no_xwayland
                    .join("bin/dwl")
                    .to_string_lossy()
                    .into_owned(),
            ],
            variables: parent_variables,
            runtime: parent_runtime,
            log: folders.root.join("parent.log"),
        }),
        runtime_link: Some(folders.short_runtime.clone()),
    })
}

/// Mutter's own headless backend, one virtual monitor at 240 Hz, its scale from its monitor
/// configuration, which the bench writes for that monitor.
fn mutter(
    configuration: Configuration,
    shell: &ShellPaths,
    folders: &Folders,
) -> Result<Plan, String> {
    let mut arguments = words(&["--headless", "--virtual-monitor", "3840x2160@240"]);
    let mut variables = Vec::new();
    let wallpaper = "its default plugin's: a plain colour with a vignette, which no switch turns \
                     off; the same in both configurations";
    let settings = match configuration {
        Configuration::AsShipped => settings(&[
            (
                "effects",
                "its default plugin's animations: workspace switch, map, minimize, destroy",
            ),
            (
                "xwayland",
                "on demand, Mutter's policy in a systemd user unit (src/core/meta-context-main.c)",
            ),
            ("wallpaper", wallpaper),
        ]),
        Configuration::EqualWork => {
            arguments.push("--no-x11".to_owned());
            variables.push(("MUTTER_DEBUG_DISABLE_ANIMATIONS".to_owned(), "1".to_owned()));
            settings(&[
                (
                    "effects",
                    "animations off: MUTTER_DEBUG_DISABLE_ANIMATIONS=1",
                ),
                ("xwayland", "off: --no-x11"),
                ("wallpaper", wallpaper),
            ])
        }
    };
    Ok(Plan {
        // nixpkgs' wrapper, which adds only store paths: its settings' schemas, among them.
        program: shell.mutter.join("bin/mutter"),
        arguments,
        path: [bin(&shell.mutter), bin(&shell.xwayland)].join(":"),
        variables,
        version: version(&shell.mutter),
        settings,
        files: vec![(folders.config().join("monitors.xml"), mutter_monitors())],
        parent: None,
        runtime_link: None,
    })
}

/// Mutter's monitor configuration for its first virtual monitor, at scale 1.5: its connector,
/// `Meta-0`, and the vendor, product and serial `--virtual-monitor` gives it
/// (`meta-output-virtual.c`, `meta-context-main.c`).
pub fn mutter_monitors() -> String {
    format!(
        "<monitors version=\"2\">\n  <configuration>\n    <logicalmonitor>\n      <x>0</x>\n      \
         <y>0</y>\n      <scale>{OUTPUT_SCALE}</scale>\n      <primary>yes</primary>\n      \
         <monitor>\n        <monitorspec>\n          <connector>Meta-0</connector>\n          \
         <vendor>MetaVendor</vendor>\n          <product>MetaVirtualMonitor</product>\n          \
         <serial>0x00</serial>\n        </monitorspec>\n        <mode>\n          \
         <width>{OUTPUT_WIDTH}</width>\n          <height>{OUTPUT_HEIGHT}</height>\n          \
         <rate>240</rate>\n        </mode>\n      </monitor>\n    </logicalmonitor>\n  \
         </configuration>\n</monitors>\n"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_block_is_removed_with_its_nested_blocks() {
        let shipped = "set $mod Mod4\nbar {\n    position top\n    colors {\n        \
                       statusline #ffffff\n    }\n}\nbindsym $mod+f fullscreen\n";
        assert_eq!(
            without_block(shipped, "bar {"),
            "set $mod Mod4\nbindsym $mod+f fullscreen\n"
        );
        assert_eq!(without_block("a\nb\n", "bar {"), "a\nb\n");
    }

    #[test]
    fn kwins_own_output_configuration_is_set_to_240_hz() {
        // As KWin 6.8.2 writes it for a virtual output at its first start (abridged).
        let written = r#"[
            {"name": "outputs", "data": [{"connectorName": "Virtual-0", "edidHash": "",
              "edidIdentifier": "", "mode": {"basic": {"height": 2160, "refreshRate": 60000,
              "width": 3840}, "flags": 1}, "scale": 1.5}]},
            {"name": "setups", "data": [{"lidClosed": false, "outputs": [{"enabled": true,
              "outputIndex": 0}]}]}
        ]"#;
        let edited: serde_json::Value =
            serde_json::from_str(&kwin_output_at_240(written).unwrap()).unwrap();
        let output = &edited[0]["data"][0];
        assert_eq!(output["mode"]["basic"]["refreshRate"], 240_000);
        assert_eq!(output["mode"]["flags"], 10);
        assert_eq!(output["customModes"][0], output["mode"]);
        assert_eq!(output["connectorName"], "Virtual-0");
        assert_eq!(edited[1]["name"], "setups");
        assert!(kwin_output_at_240("[]").is_err());
        assert!(kwin_output_at_240("not json").is_err());
    }

    #[test]
    fn every_recipe_is_found_by_its_name_and_both_configurations_by_theirs() {
        for recipe in &RECIPES {
            assert_eq!(super::recipe(recipe.name), Some(recipe));
            assert!(recipe.actions.is_empty() == recipe.no_actions.is_some());
        }
        assert_eq!(super::recipe("gamescope"), None);
        assert_eq!(
            Configuration::named("equal-work"),
            Some(Configuration::EqualWork)
        );
        assert_eq!(Configuration::named("as shipped"), None);
    }

    #[test]
    fn every_rival_of_adr_0071_has_a_recipe() {
        for rival in ADR_0071_RIVALS {
            assert!(
                recipe(rival).is_some() != tour_only(rival).is_some(),
                "{rival} has no recipe, or two"
            );
        }
        assert_eq!(TOUR_ONLY.map(|rival| rival.name), ["niri", "cosmic-comp"]);
    }

    #[test]
    fn wlroots_headless_draws_at_250_hz_for_240() {
        assert_eq!(WLROOTS_HEADLESS_HZ, 250.0);
    }

    fn shell() -> ShellPaths {
        crate::testing::shell()
    }

    fn folders(name: &str) -> Folders {
        Folders::create(&crate::testing::folder(name)).unwrap()
    }

    #[test]
    fn dwl_and_mango_run_a_build_without_xwayland_at_equal_work() {
        let shell = shell();
        let shipped = dwl(Configuration::AsShipped, &shell).unwrap();
        let equal = dwl(Configuration::EqualWork, &shell).unwrap();
        assert_eq!(shipped.program, shell.dwl.join("bin/dwl"));
        assert_eq!(equal.program, shell.dwl_no_xwayland.join("bin/dwl"));
        assert!(
            equal
                .variables
                .contains(&("WLR_BACKENDS".to_owned(), "headless".to_owned()))
        );
        assert_eq!(
            shipped.settings.keys().collect::<Vec<_>>(),
            equal.settings.keys().collect::<Vec<_>>()
        );
        // dwl's mode comes from wlr-randr once it is ready.
        assert_eq!(
            recipe("dwl").unwrap().setup,
            [Action::WlrRandr(&[
                "--output",
                "HEADLESS-1",
                "--custom-mode",
                "3840x2160@240Hz",
                "--scale",
                "1.5"
            ])]
        );
        let actions = recipe("dwl").unwrap().actions;
        assert_eq!(actions.len(), 8);
        assert!(
            actions
                .iter()
                .all(|action| matches!(action, Action::Keys(_)))
        );
    }

    #[test]
    fn mangos_configuration_is_its_own_then_the_benchs() {
        let root = crate::testing::folder("plan-mango-shell");
        let mut shell = shell();
        shell.mango = root.join("mango");
        fs::create_dir_all(shell.mango.join("etc/mango")).unwrap();
        fs::write(
            shell.mango.join("etc/mango/config.conf"),
            "animations=1\nborder_radius=6\n",
        )
        .unwrap();
        let folders = folders("plan-mango");
        let shipped = mango(Configuration::AsShipped, &shell, &folders).unwrap();
        let (path, text) = &shipped.files[0];
        assert_eq!(shipped.arguments, ["-c", &path.to_string_lossy()]);
        assert!(text.starts_with("animations=1\n"));
        assert!(
            text.contains(
                "monitor_rule=name:HEADLESS-1,width:3840,height:2160,refresh:240,scale:1.5"
            )
        );
        assert!(!text.contains("animations=0"));
        let equal = mango(Configuration::EqualWork, &shell, &folders).unwrap();
        let text = &equal.files[0].1;
        // The bench's lines come after the shipped ones, which they override.
        assert!(text.find("animations=1").unwrap() < text.find("animations=0").unwrap());
        assert!(text.contains("border_radius=0"));
        assert_eq!(equal.program, shell.mango_no_xwayland.join("bin/mango"));
    }

    #[test]
    fn umbriels_configuration_includes_its_own_and_sets_the_output() {
        let root = crate::testing::folder("plan-umbriel-shell");
        let mut shell = shell();
        shell.umbriel = root.join("umbriel");
        fs::create_dir_all(shell.umbriel.join("share/umbriel")).unwrap();
        fs::write(
            shell.umbriel.join("share/umbriel/config.toml"),
            "[general]\n",
        )
        .unwrap();
        let folders = folders("plan-umbriel");
        let shipped = umbriel(Configuration::AsShipped, &shell, &folders).unwrap();
        let text = &shipped.files[0].1;
        assert!(text.starts_with("[include]\nfiles = [\""));
        assert!(text.contains("[output.HEADLESS-1]\nmode = \"3840x2160@240\"\nscale = 1.5\n"));
        assert!(!text.contains("[animation]"));
        let equal = umbriel(Configuration::EqualWork, &shell, &folders).unwrap();
        let text = &equal.files[0].1;
        for line in [
            "xwayland = false",
            "show_cheatsheet = false",
            "corner_radius = 0",
            "[animation]\nenabled = false",
        ] {
            assert!(text.contains(line), "{line} missing from {text}");
        }
        assert!(
            umbriel(Configuration::AsShipped, &crate::testing::shell(), &folders).is_err(),
            "a package without its configuration is refused"
        );
    }

    #[test]
    fn hyprland_runs_inside_a_headless_dwl_and_draws_on_its_own_output() {
        let root = crate::testing::folder("plan-hyprland-shell");
        let mut shell = shell();
        shell.hyprland = root.join("hyprland");
        fs::create_dir_all(shell.hyprland.join("share/hypr")).unwrap();
        fs::write(
            shell.hyprland.join("share/hypr/hyprland.lua"),
            "hl.config({ animations = { enabled = true } })\n",
        )
        .unwrap();
        let folders = folders("plan-hyprland");
        let shipped = hyprland(Configuration::AsShipped, &shell, &folders).unwrap();
        let text = &shipped.files[0].1;
        assert!(text.contains(
            "hl.monitor({ output = \"HEADLESS-1\", mode = \"3840x2160@240\", position = \"0x0\", \
             scale = \"1.5\" })"
        ));
        assert!(text.contains("hl.monitor({ output = \"WAYLAND-1\", disabled = true })"));
        assert!(!text.contains("enabled = false"));
        assert!(shipped.path.contains("xwayland"));
        assert!(
            shipped
                .variables
                .contains(&("LIBSEAT_BACKEND".to_owned(), "none".to_owned()))
        );
        assert!(shipped.program.ends_with("bin/start-hyprland"));
        assert_eq!(shipped.arguments[..2], ["--", "--config"]);
        let parent = shipped.parent.as_ref().unwrap();
        assert_eq!(
            parent.command,
            [shell.dwl_no_xwayland.join("bin/dwl").to_string_lossy()]
        );
        assert!(parent.runtime.starts_with(folders.runtime()));
        assert!(
            parent
                .variables
                .contains(&("WLR_BACKENDS".to_owned(), "headless".to_owned()))
        );
        // Its runtime folder is a short link, which its environment names.
        let link = shipped.runtime_link.as_ref().unwrap();
        assert!(link.to_string_lossy().len() < 30, "{}", link.display());
        assert!(shipped.variables.contains(&(
            "XDG_RUNTIME_DIR".to_owned(),
            link.to_string_lossy().into_owned()
        )));
        assert_eq!(
            recipe("hyprland").unwrap().setup,
            [Action::Hyprctl(&[
                "output",
                "create",
                "headless",
                "HEADLESS-1"
            ])]
        );
        let equal = hyprland(Configuration::EqualWork, &shell, &folders).unwrap();
        let text = &equal.files[0].1;
        assert!(text.contains("xwayland = { enabled = false }"));
        // At equal work, no rtkit to grant realtime scheduling, as Hyprland 0.57.2 ships.
        assert!(
            equal
                .variables
                .iter()
                .any(|(key, value)| key == "DBUS_SYSTEM_BUS_ADDRESS"
                    && value.ends_with("/runtime/no-system-bus"))
        );
        assert!(
            !shipped
                .variables
                .iter()
                .any(|(key, _)| key == "DBUS_SYSTEM_BUS_ADDRESS")
        );
        assert!(equal.settings["realtime"].starts_with("none"));
        assert!(shipped.settings["realtime"].starts_with("SCHED_RR"));
        assert_eq!(
            recipe("hyprland").unwrap().restarted,
            Some("Hyprland exit not-cleanly, restarting")
        );
        assert!(text.contains("force_default_wallpaper = 0"));
        assert!(!equal.path.contains("xwayland"));
    }

    #[test]
    fn mutters_monitor_takes_the_benchs_scale() {
        let folders = folders("plan-mutter");
        let shipped = mutter(Configuration::AsShipped, &shell(), &folders).unwrap();
        assert_eq!(
            shipped.arguments,
            ["--headless", "--virtual-monitor", "3840x2160@240"]
        );
        let (path, text) = &shipped.files[0];
        assert_eq!(path, &folders.config().join("monitors.xml"));
        assert!(text.contains("<scale>1.5</scale>"));
        assert!(text.contains("<connector>Meta-0</connector>"));
        assert!(text.contains("<rate>240</rate>"));
        let equal = mutter(Configuration::EqualWork, &shell(), &folders).unwrap();
        assert_eq!(equal.arguments.last().map(String::as_str), Some("--no-x11"));
        assert!(
            equal
                .variables
                .contains(&("MUTTER_DEBUG_DISABLE_ANIMATIONS".to_owned(), "1".to_owned()))
        );
    }
}
