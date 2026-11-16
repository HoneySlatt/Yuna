//! The rivals' bench: one bench, outside every compositor, that measures each the same way
//! ([ADR-0071](../../../docs/adr/0071-the-rivals-measured-headless-by-agents-and-on-a-real-display-by-the-owner.md)).
//!
//! It starts a compositor headless, on the GPU's render node, in a sandbox of its own
//! ([`sandbox`]); runs a scenario with the same clients for every compositor ([`scenarios`]);
//! and reads its costs from outside, from what the kernel keeps of any process: the CPU time of
//! its control group ([`cgroup`]), each thread's time and wake-ups ([`procfs`], [`sampler`]),
//! the GPU's time and memory of its DRM clients ([`drm`]), its memory between windows. Beside
//! each window it reads the machine's state and the load of its other programs ([`machine`]). How
//! each compositor is run is a recipe, as data ([`recipes`]). Each run writes a versioned JSON
//! file ([`record`]); [`report`] summarises them in Markdown.
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
pub mod lock;
pub mod logs;
pub mod machine;
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
