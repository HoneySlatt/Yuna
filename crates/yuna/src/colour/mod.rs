//! Colours: what each output announces and each surface declares through `color-management-v1`
//! ([ADR-0046](../../../../docs/adr/0046-color-management-v1-kwins-list.md),
//! [ADR-0047](../../../../docs/adr/0047-hdr-virtual-outputs-pq-announced-drawn-at-stage-5.md)).
//!
//! An output announces one description for its life: the monitor's PQ volume when it is HDR,
//! gamma 2.2 and sRGB otherwise. A surface's preferred description is its window's workspace's
//! output's, whether the window is scrolled into view or not; before its window maps, its seat's
//! focused output's. Each surface's own description is recorded at its commit, for tests and
//! agents to read (`Compositor::window_colours`). Yuna draws as before: nothing is converted until
//! Stage 5.
//!
//! [`description`] holds the descriptions, with no Wayland object; `protocol.rs` the handler.

pub mod description;
pub(crate) mod protocol;

use std::collections::HashMap;
use std::sync::Arc;

use smithay::output::Output;
use smithay::reexports::wayland_protocols::wp::color_management::v1::server::{
    wp_color_management_surface_feedback_v1::WpColorManagementSurfaceFeedbackV1,
    wp_image_description_info_v1::WpImageDescriptionInfoV1,
};
use smithay::reexports::wayland_server::Resource;
use smithay::reexports::wayland_server::backend::ObjectId;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::wayland::compositor::{TraversalAction, get_role, with_surface_tree_downward};
use smithay::wayland::shell::xdg::XDG_POPUP_ROLE;
use yuna_layout::{OutputId, WindowId};

use self::description::{Description, sdr_description};
use crate::input::window_of_surface;
use crate::popups::window_surface_trees;
use crate::seats::seat_of_surface;
use crate::state::State;

/// An image description record: a description and its identity, which no other record ever
/// has, so that clients can tell copies apart from new ones.
#[derive(Debug, PartialEq, Eq)]
pub struct Record {
    /// Never 0, never reused.
    pub identity: u64,
    /// What it describes.
    pub description: Description,
}

/// A surface's colour state, double-buffered in Smithay's cached state: the description its
/// client set, if any. The intent is always perceptual, the one Yuna lists.
#[derive(Debug, Default, Clone)]
pub(crate) struct SurfaceColour {
    pub record: Option<Arc<Record>>,
}

/// What the compositor keeps for colours: the next identity, the live feedback objects, told
/// when their surface's preferred description changes, and a fallback SDR record.
#[derive(Debug)]
pub(crate) struct ColourState {
    next_identity: u64,
    /// By object, so that each one's destruction costs a constant time, however many a client
    /// holds.
    pub feedbacks: HashMap<ObjectId, WpColorManagementSurfaceFeedbackV1>,
    /// The SDR record of surfaces whose seat has no output, made when first needed.
    sdr: Option<Arc<Record>>,
    /// Information objects made during the dispatch, answered after it.
    pub pending_information: Vec<(WpImageDescriptionInfoV1, Arc<Record>)>,
}

impl ColourState {
    pub(crate) fn new() -> ColourState {
        ColourState {
            // 0 is invalid in the protocol.
            next_identity: 1,
            feedbacks: HashMap::new(),
            sdr: None,
            pending_information: Vec::new(),
        }
    }

    /// A new record of `description`, with an identity never used before.
    pub(crate) fn record(&mut self, description: Description) -> Arc<Record> {
        let identity = self.next_identity;
        self.next_identity += 1;
        Arc::new(Record {
            identity,
            description,
        })
    }
}

/// Where a surface lies in its window, for those who read its colours.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SurfaceRole {
    /// The window's own surface.
    Window,
    /// A subsurface, of the window or of one of its popups.
    Subsurface,
    /// A popup's surface.
    Popup,
}

impl SurfaceRole {
    /// Its name, as MCP gives it.
    pub fn name(self) -> &'static str {
        match self {
            SurfaceRole::Window => "window",
            SurfaceRole::Subsurface => "subsurface",
            SurfaceRole::Popup => "popup",
        }
    }
}

/// A surface of a window that declared its colours, and what it declared.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SurfaceColourDescription {
    /// Where the surface lies in its window.
    pub role: SurfaceRole,
    /// What it declared.
    pub description: Description,
}

impl State {
    /// The record an output announces, if Yuna has it.
    pub(crate) fn output_record(&self, output: &Output) -> Option<Arc<Record>> {
        self.outputs
            .iter()
            .find(|o| o.output == *output)
            .map(|o| o.colour.clone())
    }

    /// The output whose description `surface` is told to prefer: its window's workspace's,
    /// else its seat's focused output.
    fn preferred_output(&self, surface: &WlSurface) -> Option<OutputId> {
        let window = window_of_surface(surface)
            .and_then(|root| self.toplevels.get(&root.id()))
            .and_then(|toplevel| toplevel.window);
        window
            .and_then(|window| self.layout.window(window))
            .and_then(|info| info.output)
            .or_else(|| self.layout.active_output(seat_of_surface(surface)))
    }

    /// The description `surface` is told to prefer; an SDR one when its seat has no output.
    pub(crate) fn preferred_record(&mut self, surface: &WlSurface) -> Arc<Record> {
        if let Some(record) = self
            .preferred_output(surface)
            .and_then(|id| self.output_index(id))
            .map(|index| self.outputs[index].colour.clone())
        {
            return record;
        }
        self.colour.sdr_without_output()
    }

    /// Answers the information objects made during the last dispatch.
    pub(crate) fn send_pending_information(&mut self) {
        for (info, record) in std::mem::take(&mut self.colour.pending_information) {
            if info.is_alive() {
                protocol::send_information(&info, &record.description);
            }
        }
    }

    /// Tells each feedback object whose surface's preferred description changed. Called after
    /// each arrangement, which moves windows. A feedback object whose surface is gone is inert:
    /// it stays until its client destroys it.
    pub(crate) fn update_preferred_descriptions(&mut self) {
        let feedbacks: Vec<WpColorManagementSurfaceFeedbackV1> =
            self.colour.feedbacks.values().cloned().collect();
        for feedback in feedbacks {
            if let Some((surface, told)) = protocol::feedback_state(&feedback) {
                let identity = self.preferred_record(&surface).identity;
                if identity != told {
                    protocol::tell_preferred_changed(&feedback, identity);
                    protocol::set_told(&feedback, identity);
                }
            }
        }
    }

    /// The colours the surfaces of `window` declared, at their last commit: its own surface's,
    /// its subsurfaces' and its popups', in that order; those that declared none are left out.
    pub(crate) fn window_colours(&self, window: WindowId) -> Vec<SurfaceColourDescription> {
        let Some(root) = self
            .windows
            .get(&window)
            .and_then(|id| self.toplevels.get(id))
            .map(|toplevel| toplevel.wl_surface().clone())
        else {
            return Vec::new();
        };
        let mut found = Vec::new();
        for tree in window_surface_trees(&root) {
            let tree_role = if get_role(&tree) == Some(XDG_POPUP_ROLE) {
                SurfaceRole::Popup
            } else {
                SurfaceRole::Window
            };
            with_surface_tree_downward(
                &tree,
                (),
                |_, _, _| TraversalAction::DoChildren(()),
                |surface, states, _| {
                    // The tree's root is the window's or the popup's surface; the rest are
                    // subsurfaces.
                    let role = if *surface == tree {
                        tree_role
                    } else {
                        SurfaceRole::Subsurface
                    };
                    let colour = states.cached_state.get::<SurfaceColour>().current().clone();
                    if let Some(record) = colour.record {
                        found.push(SurfaceColourDescription {
                            role,
                            description: record.description.clone(),
                        });
                    }
                },
                |_, _, _| true,
            );
        }
        found
    }
}

impl ColourState {
    /// The SDR record for a surface whose seat has no output, made once.
    fn sdr_without_output(&mut self) -> Arc<Record> {
        if let Some(record) = &self.sdr {
            return record.clone();
        }
        let record = self.record(sdr_description());
        self.sdr = Some(record.clone());
        record
    }
}
