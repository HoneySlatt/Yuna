//! `color-management-v1` on wayland-server: Yuna's own handler, not Smithay's
//! ([ADR-0039](../../../../docs/adr/0039-leaving-smithay-step-by-step.md),
//! [ADR-0046](../../../../docs/adr/0046-color-management-v1-kwins-list.md)).
//!
//! Each object's state lives in its own user data; Yuna keeps no registry of descriptions, so
//! that a client's descriptions cost it only its own objects. Every event goes out only at the
//! version the client bound: `ready2` and `preferred_changed2` from version 2, `ready` and
//! `preferred_changed` before. A surface's description is double-buffered in Smithay's cached
//! state ([`SurfaceColour`]) until Yuna's own core holds surfaces.

use std::sync::{Arc, Mutex};

use smithay::output::Output;
use smithay::reexports::wayland_protocols::wp::color_management::v1::server::{
    wp_color_management_output_v1::{self, WpColorManagementOutputV1},
    wp_color_management_surface_feedback_v1::{self, WpColorManagementSurfaceFeedbackV1},
    wp_color_management_surface_v1::{self, WpColorManagementSurfaceV1},
    wp_color_manager_v1::{self, WpColorManagerV1},
    wp_image_description_creator_icc_v1::{self, WpImageDescriptionCreatorIccV1},
    wp_image_description_creator_params_v1::{self, WpImageDescriptionCreatorParamsV1},
    wp_image_description_info_v1::{self, WpImageDescriptionInfoV1},
    wp_image_description_reference_v1::{self, WpImageDescriptionReferenceV1},
    wp_image_description_v1::{self, WpImageDescriptionV1},
};
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::reexports::wayland_server::{
    Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource, WEnum, Weak,
};
use smithay::wayland::compositor::{Cacheable, with_states};

use super::description::{
    Chromaticities, Description, Luminances, NamedPrimaries, Params, ParamsError, TransferFunction,
};
use super::{Record, SurfaceColour};
use crate::state::State;

/// The version Yuna offers: wayland-protocols 0.32.13's.
pub const VERSION: u32 = 3;

/// The global's data.
pub(crate) struct ManagerGlobal;

/// A surface's mark that it has a `wp_color_management_surface_v1`: at most one may live.
#[derive(Default)]
struct HasColourSurface(Mutex<bool>);

/// What a `wp_image_description_v1` refers to: a record, ready, or nothing, having failed.
pub(crate) enum ImageDescription {
    Ready {
        record: Arc<Record>,
        /// Whether `get_information` is allowed: for an output's or a preferred description,
        /// not for one a client made.
        information: bool,
    },
    Failed,
}

/// A feedback object's surface, and the identity it was last told.
pub(crate) struct Feedback {
    surface: Weak<WlSurface>,
    told: Mutex<u64>,
}

impl Cacheable for SurfaceColour {
    fn commit(&mut self, _dh: &DisplayHandle) -> SurfaceColour {
        self.clone()
    }

    fn merge_into(self, into: &mut SurfaceColour, _dh: &DisplayHandle) {
        *into = self;
    }
}

/// Creates the global.
pub(crate) fn manager_global(display: &DisplayHandle) {
    display.create_global::<State, WpColorManagerV1, _>(VERSION, ManagerGlobal);
}

impl GlobalDispatch<WpColorManagerV1, ManagerGlobal> for State {
    fn bind(
        _state: &mut State,
        _display: &DisplayHandle,
        _client: &Client,
        resource: New<WpColorManagerV1>,
        _global: &ManagerGlobal,
        data_init: &mut DataInit<'_, State>,
    ) {
        let manager = data_init.init(resource, ());
        let version = manager.version();
        manager.supported_intent(wp_color_manager_v1::RenderIntent::Perceptual);
        for feature in [
            wp_color_manager_v1::Feature::Parametric,
            wp_color_manager_v1::Feature::SetPrimaries,
            wp_color_manager_v1::Feature::SetLuminances,
            wp_color_manager_v1::Feature::SetMasteringDisplayPrimaries,
        ] {
            manager.supported_feature(feature);
        }
        for tf in TransferFunction::ALL {
            if version >= tf.since()
                && let Ok(tf) = wp_color_manager_v1::TransferFunction::try_from(tf.wire())
            {
                manager.supported_tf_named(tf);
            }
        }
        for primaries in NamedPrimaries::ALL {
            if let Ok(primaries) = wp_color_manager_v1::Primaries::try_from(primaries.wire()) {
                manager.supported_primaries_named(primaries);
            }
        }
        manager.done();
    }
}

impl Dispatch<WpColorManagerV1, ()> for State {
    fn request(
        state: &mut State,
        _client: &Client,
        manager: &WpColorManagerV1,
        request: wp_color_manager_v1::Request,
        _data: &(),
        _display: &DisplayHandle,
        data_init: &mut DataInit<'_, State>,
    ) {
        use wp_color_manager_v1::{Error, Request};
        match request {
            Request::GetOutput { id, output } => {
                // Inert from the start when the output is gone already.
                let output = Output::from_resource(&output);
                data_init.init(id, output.map(|o| o.downgrade()));
            }
            Request::GetSurface { id, surface } => {
                let taken = with_states(&surface, |states| {
                    let mark = states
                        .data_map
                        .get_or_insert_threadsafe(HasColourSurface::default);
                    let mut taken = mark.0.lock().unwrap_or_else(|e| e.into_inner());
                    std::mem::replace(&mut *taken, true)
                });
                if taken {
                    manager.post_error(
                        Error::SurfaceExists,
                        "the surface has a colour management surface already",
                    );
                    return;
                }
                data_init.init(id, surface.downgrade());
            }
            Request::GetSurfaceFeedback { id, surface } => {
                // What it would be told first: a change from it is what it is told later.
                let identity = state.preferred_record(&surface).identity;
                let feedback = data_init.init(
                    id,
                    Feedback {
                        surface: surface.downgrade(),
                        told: Mutex::new(identity),
                    },
                );
                state.colour.feedbacks.insert(feedback.id(), feedback);
            }
            Request::CreateParametricCreator { obj } => {
                data_init.init(obj, Mutex::new(Params::default()));
            }
            Request::CreateIccCreator { .. } => {
                manager.post_error(Error::UnsupportedFeature, "Yuna does not take ICC profiles")
            }
            Request::CreateWindowsScrgb { .. } | Request::CreateWindowsBt2100 { .. } => manager
                .post_error(
                    Error::UnsupportedFeature,
                    "Yuna does not list Windows' descriptions",
                ),
            Request::GetImageDescription {
                image_description, ..
            } => {
                // Yuna makes no reference: one can only come from another protocol, which Yuna
                // does not offer.
                let description = data_init.init(image_description, ImageDescription::Failed);
                description.failed(
                    wp_image_description_v1::Cause::Unsupported,
                    "Yuna knows no reference".to_owned(),
                );
            }
            Request::Destroy => {}
            _ => {}
        }
    }
}

impl Dispatch<WpColorManagementOutputV1, Option<smithay::output::WeakOutput>> for State {
    fn request(
        state: &mut State,
        _client: &Client,
        _object: &WpColorManagementOutputV1,
        request: wp_color_management_output_v1::Request,
        output: &Option<smithay::output::WeakOutput>,
        _display: &DisplayHandle,
        data_init: &mut DataInit<'_, State>,
    ) {
        if let wp_color_management_output_v1::Request::GetImageDescription { image_description } =
            request
        {
            // Inert once Yuna removed the output, at once, not when its global goes.
            let record = output
                .as_ref()
                .and_then(|weak| weak.upgrade())
                .and_then(|output| state.output_record(&output));
            match record {
                Some(record) => ready(data_init, image_description, record, true),
                None => {
                    let description = data_init.init(image_description, ImageDescription::Failed);
                    description.failed(
                        wp_image_description_v1::Cause::NoOutput,
                        "the output is gone".to_owned(),
                    );
                }
            }
        }
    }
}

impl Dispatch<WpColorManagementSurfaceV1, Weak<WlSurface>> for State {
    fn request(
        _state: &mut State,
        _client: &Client,
        object: &WpColorManagementSurfaceV1,
        request: wp_color_management_surface_v1::Request,
        surface: &Weak<WlSurface>,
        _display: &DisplayHandle,
        _data_init: &mut DataInit<'_, State>,
    ) {
        use wp_color_management_surface_v1::{Error, Request};
        let Ok(surface) = surface.upgrade() else {
            if !matches!(request, Request::Destroy) {
                object.post_error(Error::Inert, "the surface is gone");
            }
            return;
        };
        let pending = match request {
            Request::SetImageDescription {
                image_description,
                render_intent,
            } => {
                let Some(ImageDescription::Ready { record, .. }) =
                    image_description.data::<ImageDescription>()
                else {
                    object.post_error(Error::ImageDescription, "the description is not ready");
                    return;
                };
                if render_intent != WEnum::Value(wp_color_manager_v1::RenderIntent::Perceptual) {
                    object.post_error(
                        Error::RenderIntent,
                        "Yuna lists the perceptual intent alone",
                    );
                    return;
                }
                Some(record.clone())
            }
            Request::UnsetImageDescription => None,
            Request::Destroy => {
                with_states(&surface, |states| {
                    if let Some(mark) = states.data_map.get::<HasColourSurface>() {
                        *mark.0.lock().unwrap_or_else(|e| e.into_inner()) = false;
                    }
                });
                None
            }
            _ => return,
        };
        with_states(&surface, |states| {
            states.cached_state.get::<SurfaceColour>().pending().record = pending;
        });
    }
}

impl Dispatch<WpColorManagementSurfaceFeedbackV1, Feedback> for State {
    fn request(
        state: &mut State,
        _client: &Client,
        object: &WpColorManagementSurfaceFeedbackV1,
        request: wp_color_management_surface_feedback_v1::Request,
        feedback: &Feedback,
        _display: &DisplayHandle,
        data_init: &mut DataInit<'_, State>,
    ) {
        use wp_color_management_surface_feedback_v1::{Error, Request};
        let image_description = match request {
            Request::GetPreferred { image_description }
            | Request::GetPreferredParametric { image_description } => image_description,
            _ => return,
        };
        let Ok(surface) = feedback.surface.upgrade() else {
            object.post_error(Error::Inert, "the surface is gone");
            return;
        };
        let record = state.preferred_record(&surface);
        ready(data_init, image_description, record, true);
    }

    fn destroyed(
        state: &mut State,
        _client: smithay::reexports::wayland_server::backend::ClientId,
        object: &WpColorManagementSurfaceFeedbackV1,
        _feedback: &Feedback,
    ) {
        state.colour.feedbacks.remove(&object.id());
    }
}

impl Dispatch<WpImageDescriptionCreatorParamsV1, Mutex<Params>> for State {
    fn request(
        state: &mut State,
        _client: &Client,
        creator: &WpImageDescriptionCreatorParamsV1,
        request: wp_image_description_creator_params_v1::Request,
        params: &Mutex<Params>,
        _display: &DisplayHandle,
        data_init: &mut DataInit<'_, State>,
    ) {
        use wp_image_description_creator_params_v1::Request;
        let mut params = params.lock().unwrap_or_else(|e| e.into_inner());
        let result = match request {
            Request::SetTfNamed { tf } => {
                params.set_transfer_function(u32::from(tf), creator.version())
            }
            Request::SetTfPower { .. } => Err(ParamsError::UnsupportedFeature),
            Request::SetPrimariesNamed { primaries } => {
                params.set_named_primaries(u32::from(primaries))
            }
            Request::SetPrimaries {
                r_x,
                r_y,
                g_x,
                g_y,
                b_x,
                b_y,
                w_x,
                w_y,
            } => params.set_primaries(Chromaticities::from_values([
                r_x, r_y, g_x, g_y, b_x, b_y, w_x, w_y,
            ])),
            Request::SetLuminances {
                min_lum,
                max_lum,
                reference_lum,
            } => params.set_luminances(Luminances {
                min: min_lum,
                max: max_lum,
                reference: reference_lum,
            }),
            Request::SetMasteringDisplayPrimaries {
                r_x,
                r_y,
                g_x,
                g_y,
                b_x,
                b_y,
                w_x,
                w_y,
            } => params.set_target_primaries(Chromaticities::from_values([
                r_x, r_y, g_x, g_y, b_x, b_y, w_x, w_y,
            ])),
            Request::SetMasteringLuminance { min_lum, max_lum } => {
                params.set_target_luminance(min_lum, max_lum)
            }
            Request::SetMaxCll { max_cll } => {
                params.set_max_cll(max_cll);
                Ok(())
            }
            Request::SetMaxFall { max_fall } => {
                params.set_max_fall(max_fall);
                Ok(())
            }
            Request::Create { image_description } => {
                match std::mem::take(&mut *params).create() {
                    Ok(description) => {
                        let record = state.colour.record(description);
                        ready(data_init, image_description, record, false);
                        Ok(())
                    }
                    Err(ParamsError::Unusable) => {
                        let description =
                            data_init.init(image_description, ImageDescription::Failed);
                        description.failed(
                            wp_image_description_v1::Cause::Unsupported,
                            ParamsError::Unusable.to_string(),
                        );
                        Ok(())
                    }
                    Err(error) => {
                        // The new object is made, so that the error names a live one.
                        data_init.init(image_description, ImageDescription::Failed);
                        Err(error)
                    }
                }
            }
            _ => Ok(()),
        };
        if let Err(error) = result {
            use wp_image_description_creator_params_v1::Error;
            let code = match error {
                ParamsError::IncompleteSet => Error::IncompleteSet,
                ParamsError::AlreadySet => Error::AlreadySet,
                ParamsError::UnsupportedFeature => Error::UnsupportedFeature,
                ParamsError::InvalidTf => Error::InvalidTf,
                ParamsError::InvalidPrimariesNamed => Error::InvalidPrimariesNamed,
                ParamsError::InvalidLuminance | ParamsError::Unusable => Error::InvalidLuminance,
            };
            creator.post_error(code, error.to_string());
        }
    }
}

impl Dispatch<WpImageDescriptionV1, ImageDescription> for State {
    fn request(
        state: &mut State,
        _client: &Client,
        object: &WpImageDescriptionV1,
        request: wp_image_description_v1::Request,
        description: &ImageDescription,
        _display: &DisplayHandle,
        data_init: &mut DataInit<'_, State>,
    ) {
        use wp_image_description_v1::{Error, Request};
        if let Request::GetInformation { information } = request {
            match description {
                ImageDescription::Failed => {
                    object.post_error(Error::NotReady, "the description failed");
                }
                ImageDescription::Ready {
                    information: false, ..
                } => object.post_error(
                    Error::NoInformation,
                    "a client's own description gives no information",
                ),
                ImageDescription::Ready { record, .. } => {
                    // Sent after the dispatch: its last event, `done`, destroys the object, and
                    // wayland-backend 0.3.17 panics when a request's new object is destroyed
                    // before the request returns (`server_impl/common_poll.rs:284`).
                    let info = data_init.init(information, ());
                    state
                        .colour
                        .pending_information
                        .push((info, record.clone()));
                }
            }
        }
    }
}

impl Dispatch<WpImageDescriptionInfoV1, ()> for State {
    fn request(
        _state: &mut State,
        _client: &Client,
        _object: &WpImageDescriptionInfoV1,
        _request: wp_image_description_info_v1::Request,
        _data: &(),
        _display: &DisplayHandle,
        _data_init: &mut DataInit<'_, State>,
    ) {
    }
}

impl Dispatch<WpImageDescriptionCreatorIccV1, ()> for State {
    fn request(
        _state: &mut State,
        _client: &Client,
        _object: &WpImageDescriptionCreatorIccV1,
        _request: wp_image_description_creator_icc_v1::Request,
        _data: &(),
        _display: &DisplayHandle,
        _data_init: &mut DataInit<'_, State>,
    ) {
        // Never made: `create_icc_creator` is refused, Yuna listing no ICC.
    }
}

impl Dispatch<WpImageDescriptionReferenceV1, ()> for State {
    fn request(
        _state: &mut State,
        _client: &Client,
        _object: &WpImageDescriptionReferenceV1,
        _request: wp_image_description_reference_v1::Request,
        _data: &(),
        _display: &DisplayHandle,
        _data_init: &mut DataInit<'_, State>,
    ) {
        // Never made by Yuna: no protocol it offers creates references.
    }
}

/// Makes `new` a ready description of `record`, and says so at the client's version.
fn ready(
    data_init: &mut DataInit<'_, State>,
    new: New<WpImageDescriptionV1>,
    record: Arc<Record>,
    information: bool,
) {
    let identity = record.identity;
    let description = data_init.init(
        new,
        ImageDescription::Ready {
            record,
            information,
        },
    );
    if description.version() >= 2 {
        let (hi, lo) = split(identity);
        description.ready2(hi, lo);
    } else {
        description.ready(low_identity(identity));
    }
}

/// Tells a feedback object its surface's preferred description changed, at its version.
pub(crate) fn tell_preferred_changed(feedback: &WpColorManagementSurfaceFeedbackV1, identity: u64) {
    if feedback.version() >= 2 {
        let (hi, lo) = split(identity);
        feedback.preferred_changed2(hi, lo);
    } else {
        feedback.preferred_changed(low_identity(identity));
    }
}

/// The surface a feedback object is for, and the identity it was last told.
pub(crate) fn feedback_state(
    feedback: &WpColorManagementSurfaceFeedbackV1,
) -> Option<(WlSurface, u64)> {
    let data = feedback.data::<Feedback>()?;
    let surface = data.surface.upgrade().ok()?;
    let told = *data.told.lock().unwrap_or_else(|e| e.into_inner());
    Some((surface, told))
}

/// Records that a feedback object was told `identity`.
pub(crate) fn set_told(feedback: &WpColorManagementSurfaceFeedbackV1, identity: u64) {
    if let Some(data) = feedback.data::<Feedback>() {
        *data.told.lock().unwrap_or_else(|e| e.into_inner()) = identity;
    }
}

/// An identity's high and low 32 bits, as `ready2` and `preferred_changed2` take them.
fn split(identity: u64) -> (u32, u32) {
    ((identity >> 32) as u32, identity as u32)
}

/// A 32-bit identity for version 1, which may recycle them; never 0, which is invalid.
fn low_identity(identity: u64) -> u32 {
    (identity as u32).max(1)
}

/// Sends every event that describes `description`, then `done`, which destroys `info`.
pub(crate) fn send_information(info: &WpImageDescriptionInfoV1, description: &Description) {
    let [r_x, r_y, g_x, g_y, b_x, b_y, w_x, w_y] = description.primaries.values();
    info.primaries(r_x, r_y, g_x, g_y, b_x, b_y, w_x, w_y);
    if let Some(named) = description.named_primaries
        && let Ok(named) = wp_color_manager_v1::Primaries::try_from(named.wire())
    {
        info.primaries_named(named);
    }
    if let Ok(tf) =
        wp_color_manager_v1::TransferFunction::try_from(description.transfer_function.wire())
    {
        info.tf_named(tf);
    }
    let luminances = description.luminances;
    info.luminances(luminances.min, luminances.max, luminances.reference);
    let [r_x, r_y, g_x, g_y, b_x, b_y, w_x, w_y] = description.target_primaries.values();
    info.target_primaries(r_x, r_y, g_x, g_y, b_x, b_y, w_x, w_y);
    let (min, max) = description.target_luminance;
    info.target_luminance(min, max);
    if let Some(max_cll) = description.max_cll {
        info.target_max_cll(max_cll);
    }
    if let Some(max_fall) = description.max_fall {
        info.target_max_fall(max_fall);
    }
    info.done();
}
