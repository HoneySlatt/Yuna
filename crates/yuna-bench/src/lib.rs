//! The rivals' bench: one bench, outside every compositor, that measures each the same way
//! ([ADR-0075](../../../docs/adr/0075-the-rivals-measured-headless-now-on-a-real-display-at-stage-2s-exit.md)).
//!
//! It starts a compositor headless, on the GPU's render node, in a sandbox of its own
//! ([`sandbox`]); runs a scenario with the same clients for every compositor ([`scenarios`]);
//! and reads its costs from outside, from what the kernel keeps of any process: the CPU time of
//! its control group ([`cgroup`]), each thread's time and wake-ups ([`procfs`], [`sampler`]),
//! the GPU's time and memory of its DRM clients ([`drm`]), its memory between windows. Beside
//! each window it reads the machine's state and the load of its other programs ([`machine`]). How
//! each compositor is run is a recipe, as data ([`recipes`]); one driven by its keys alone gets
//! the bench's virtual keyboard ([`keys`]). Each run writes a versioned JSON file ([`record`]);
//! [`report`] summarises them in Markdown.
//!
//! Beside the bench, [`parity`] compares two builds of Yuna: in CI, on figures the machine's load
//! does not move; once per milestone, on the GPU's memory and on timed figures, judged over pairs
//! ([ADR-0142](../../../docs/adr/0142-parity-a-pinned-baseline-figures-in-classes-three-verdicts.md)).
//!
//! It depends on nothing of the `yuna` crate: it measures Yuna as it measures the others, and
//! builds alone.
//!
//! The binary's commands are in [`cli`]; `CONTRIBUTING.md` says how to run it.

pub mod capture;
pub mod cgroup;
pub mod cli;
pub mod clock;
pub mod drm;
pub mod keys;
pub mod lock;
pub mod logs;
pub mod machine;
pub mod parity;
pub mod procfs;
pub mod recipes;
pub mod record;
pub mod report;
pub mod sampler;
pub mod sandbox;
pub mod scenarios;
pub mod sockets;
pub mod still;

/// What the tests share.
#[cfg(test)]
mod testing {
    use std::path::PathBuf;

    use crate::sandbox::ShellPaths;

    /// The bench shell's store paths, made up: none of them exists.
    pub fn shell() -> ShellPaths {
        let store = |name: &str| PathBuf::from(format!("/nix/store/aaaa-{name}"));
        ShellPaths {
            sway: store("sway-unwrapped-1.12"),
            kwin: store("kwin-6.8.2"),
            dwl: store("dwl-0.9"),
            dwl_no_xwayland: store("bbbb-dwl-0.9"),
            mango: store("mango-0.18.4"),
            mango_no_xwayland: store("bbbb-mango-0.18.4"),
            umbriel: store("umbriel-0.1.0"),
            hyprland: store("hyprland-0.57.1"),
            mutter: store("mutter-51.0"),
            wlr_randr: store("wlr-randr-0.5.0"),
            weston: store("weston-16.0.0"),
            ffmpeg: store("ffmpeg-headless-9.0.2-bin"),
            dbus: store("dbus-1.16.2"),
            mesa: store("mesa-26.2.6"),
            xwayland: store("xwayland-24.1.14"),
            coreutils: store("coreutils-9.8"),
            bash: store("bash-5.3"),
            applications_path: "/nix/store/aaaa-vulkan-tools/bin:/nix/store/aaaa-mpv/bin"
                .to_owned(),
            fonts_conf: store("yuna-fonts.conf"),
            xkb_config_root: store("xkeyboard-config/share/X11/xkb"),
            library_path: "/nix/store/aaaa-libglvnd/lib".to_owned(),
            egl_vendor: store("mesa-26.2.6/share/glvnd/egl_vendor.d/50_mesa.json"),
        }
    }

    /// A new empty folder for one test, in the temporary folder.
    pub fn folder(name: &str) -> PathBuf {
        let folder = std::env::temp_dir()
            .join(format!("yuna-bench-tests-{}", std::process::id()))
            .join(name);
        let _ = std::fs::remove_dir_all(&folder);
        std::fs::create_dir_all(&folder).expect("a test's folder");
        folder
    }
}
