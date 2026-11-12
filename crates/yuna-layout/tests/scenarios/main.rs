//! Scenarios: one test per behaviour of the layout model, each a short story with exact figures.
//!
//! Behaviours read in niri's documentation and tests are rewritten here, never copied
//! (ADR-0013); each such test names its source in niri 26.04 as `niri: path:line`. Hyprland's
//! behaviours name the page of its wiki or its source they were read in
//! ([ADR-0019](../../../../docs/adr/0019-the-layouts-behaviours.md)).

mod clients;
mod dwindle;
mod floating;
mod fullscreen;
mod harness;
mod outputs;
mod robustness;
mod scrolling;
mod seats;
mod special_and_pinned;
mod umbriel;
mod workspaces;
