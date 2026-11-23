//! Scenarios: one test per behaviour of the layout model, each a short story with exact figures.
//!
//! Behaviours read in niri's documentation and tests are rewritten here, never copied
//! (ADR-0013); each such test names its source in niri 26.04 as `niri: path:line`. Hyprland's
//! behaviours name the page of its wiki or its source they were read in
//! ([ADR-0019](../../../../docs/adr/0019-the-layouts-behaviours.md)).

mod clients;
mod dwindle;
mod dwindle_focus;
mod floating;
mod fullscreen;
mod harness;
mod ids;
mod output_moves;
mod outputs;
mod peek;
mod refusals;
mod robustness;
mod scrolling;
mod seats;
mod size_on_open;
mod special_and_pinned;
mod strip_columns;
mod strip_view;
mod umbriel;
mod window_states;
mod workspaces;
