//! Links the integration module `NODELETE`, so that no `dlclose` ever unmaps it.
//!
//! WLCS closes the module as its process exits, while threads the module started may still run
//! its code: Smithay starts its thread that frees `wl_shm` pools at the first pool freed, which
//! is when a compositor stops, and under load that thread was still starting as the module was
//! unmapped, which crashed the process after its test had passed. A module that stays mapped
//! until the process ends leaves such a thread nothing to fall into. `scripts/check.sh`'s `wlcs`
//! step checks the flag on the module it built.

fn main() {
    println!("cargo:rustc-cdylib-link-arg=-Wl,-z,nodelete");
}
