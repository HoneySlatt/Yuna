//! How each compositor is run: one recipe each, as data. A recipe gives the compositor's
//! program, its environment, the configuration file it writes into the run's folder, how the
//! bench tells it is ready, its version (from its store path, or Yuna's commit: never by running
//! it), how it is stopped, what it is told to do in the `motion` scenario, and how its own frames
//! are counted.
//!
//! Each recipe has two configurations: *as shipped*, its defaults, and *equal work*, its effects,
//! wallpaper and XWayland off ([ADR-0072](../../../docs/adr/0072-budgets-set-against-the-peers-the-same-day.md)).
//! [`Plan::settings`] records what differs between them.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use crate::procfs;
use crate::sandbox::{Folders, RENDER_NODE, ShellPaths};

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
}

/// A command sent to a compositor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// A sway command, through `swaymsg`.
    Sway(&'static str),
    /// A method of `org.kde.KWin` on `/KWin`, through `dbus-send`, with its arguments.
    KwinBus(&'static str, &'static [&'static str]),
}

/// One compositor's recipe.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Recipe {
    /// Its name on the command line.
    pub name: &'static str,
    /// The backend it runs on headless.
    pub backend: &'static str,
    /// What it draws with.
    pub renderer: &'static str,
    /// How its frames are counted.
    pub frames: Frames,
    /// How it tells it is ready.
    pub ready: Ready,
    /// The command that stops it; without one, SIGTERM.
    pub stop: Option<Action>,
    /// The `motion` scenario's actions, each run once the last settled; empty if it takes none.
    pub actions: &'static [Action],
    /// Why it has no actions, when it has none.
    pub no_actions: Option<&'static str>,
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

/// The recipes, by name.
pub const RECIPES: [Recipe; 4] = [
    Recipe {
        name: "sway",
        backend: "wlroots-headless",
        renderer: "gles2",
        frames: Frames::Capture,
        ready: Ready::Swaymsg,
        stop: Some(Action::Sway("exit")),
        actions: SWAY_ACTIONS,
        no_actions: None,
    },
    Recipe {
        name: "sway-vulkan",
        backend: "wlroots-headless",
        renderer: "vulkan",
        frames: Frames::Capture,
        ready: Ready::Swaymsg,
        stop: Some(Action::Sway("exit")),
        actions: SWAY_ACTIONS,
        no_actions: None,
    },
    Recipe {
        name: "kwin",
        backend: "kwin-virtual",
        renderer: "gles (KWin's virtual backend draws with OpenGL ES alone)",
        frames: Frames::KwinLog,
        ready: Ready::BusName("org.kde.KWin"),
        stop: None,
        actions: &[
            Action::KwinBus("setCurrentDesktop", &["int32:2"]),
            Action::KwinBus("setCurrentDesktop", &["int32:1"]),
        ],
        no_actions: None,
    },
    Recipe {
        name: "yuna",
        backend: "yuna-headless",
        renderer: "gles2 (Smithay's GLES renderer on the render node, --gpu)",
        // Its `frame drawn` lines need its debug logging, which would weigh on its costs.
        frames: Frames::Capture,
        ready: Ready::Line("WAYLAND_DISPLAY="),
        stop: None,
        actions: &[],
        no_actions: Some(
            "`yuna --headless` takes no command from outside; MCP's layout tools do not exist \
             (ADR-0056)",
        ),
    },
];

/// The recipe named `name`.
pub fn recipe(name: &str) -> Option<&'static Recipe> {
    RECIPES.iter().find(|recipe| recipe.name == name)
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
        variables: vec![
            ("WLR_BACKENDS".to_owned(), "headless".to_owned()),
            ("WLR_HEADLESS_OUTPUTS".to_owned(), "1".to_owned()),
            ("WLR_RENDER_DRM_DEVICE".to_owned(), RENDER_NODE.to_owned()),
            ("WLR_RENDERER".to_owned(), recipe.renderer.to_owned()),
        ],
        version: procfs::store_version(&shell.sway.to_string_lossy()).map(str::to_owned),
        settings,
        files: vec![(config, text)],
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
    let mut arguments: Vec<String> = [
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
    ]
    .iter()
    .map(|word| (*word).to_owned())
    .collect();
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
        version: procfs::store_version(&shell.kwin.to_string_lossy()).map(str::to_owned),
        settings,
        files,
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
        arguments: ["--headless", "--gpu", "--output", OUTPUT]
            .iter()
            .map(|word| (*word).to_owned())
            .collect(),
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
    })
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
        assert_eq!(super::recipe("hyprland"), None);
        assert_eq!(
            Configuration::named("equal-work"),
            Some(Configuration::EqualWork)
        );
        assert_eq!(Configuration::named("as shipped"), None);
    }
}
