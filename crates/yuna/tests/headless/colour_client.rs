//! A client of `color-management-v1`, for tests: it binds the manager at a version the test
//! chooses, and every `wl_output`, and records every event the protocol sends: what the manager
//! lists, each image description's `ready` or `failed`, the information asked of it, and each
//! feedback object's `preferred_changed`.
//!
//! A [`ColourClient`] is a test [`Client`] with these objects on an event queue of their own,
//! over the same connection, as `capture_client.rs` does.

use wayland_client::backend::WaylandError;
use wayland_client::backend::protocol::{Argument, Message};
use wayland_client::protocol::{wl_output, wl_registry, wl_surface::WlSurface};
use wayland_client::{Dispatch, DispatchError, EventQueue, Proxy, QueueHandle, WEnum};
use wayland_protocols::wp::color_management::v1::client::{
    wp_color_management_output_v1, wp_color_management_surface_feedback_v1,
    wp_color_management_surface_v1, wp_color_manager_v1, wp_image_description_creator_icc_v1,
    wp_image_description_creator_params_v1, wp_image_description_info_v1, wp_image_description_v1,
};
use yuna::Compositor;
use yuna::yuna_layout::SeatId;

use crate::client::Client;

pub use wp_color_management_surface_v1::WpColorManagementSurfaceV1 as ColourSurface;
pub use wp_color_manager_v1::{Feature, Primaries, RenderIntent, TransferFunction};
pub use wp_image_description_creator_params_v1::WpImageDescriptionCreatorParamsV1 as Params;

/// What the manager listed when it was bound.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Listed {
    pub intents: Vec<u32>,
    pub features: Vec<u32>,
    pub transfer_functions: Vec<u32>,
    pub primaries: Vec<u32>,
    pub done: bool,
}

/// What an image description was told, and the information asked of it.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct DescriptionEvents {
    /// `ready`'s 32-bit identity, or `ready2`'s 64-bit one.
    pub ready: Option<u64>,
    /// Whether it came through `ready2`.
    pub ready2: bool,
    pub failed: Option<(u32, String)>,
    pub information: Option<Information>,
}

/// The information events of one `get_information`, in order of arrival.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Information {
    pub primaries: Option<[i32; 8]>,
    pub primaries_named: Option<u32>,
    pub tf_named: Option<u32>,
    pub luminances: Option<(u32, u32, u32)>,
    pub target_primaries: Option<[i32; 8]>,
    pub target_luminance: Option<(u32, u32)>,
    pub target_max_cll: Option<u32>,
    pub target_max_fall: Option<u32>,
    pub done: bool,
}

/// A test client of `color-management-v1`.
pub struct ColourClient {
    /// Its windows' side.
    pub client: Client,
    queue: EventQueue<ColourState>,
    pub state: ColourState,
}

/// What the client's colour objects hold and received.
#[derive(Default)]
pub struct ColourState {
    version: u32,
    pub manager: Option<wp_color_manager_v1::WpColorManagerV1>,
    pub listed: Listed,
    /// Every `wl_output`, in the order the registry gave them.
    pub outputs: Vec<wl_output::WlOutput>,
    descriptions: Vec<(
        wp_image_description_v1::WpImageDescriptionV1,
        DescriptionEvents,
    )>,
    /// Each feedback object, and the identities its `preferred_changed` gave, in order.
    feedbacks: Vec<(
        wp_color_management_surface_feedback_v1::WpColorManagementSurfaceFeedbackV1,
        Vec<u64>,
    )>,
}

impl ColourClient {
    /// Connects a client of `seat`, binding the manager at `version`, and its `wl_output`s.
    pub fn connect(compositor: &mut Compositor, seat: SeatId, version: u32) -> ColourClient {
        let client = Client::connect_to(compositor, seat);
        let queue = client.connection().new_event_queue();
        client
            .connection()
            .display()
            .get_registry(&queue.handle(), ());
        let mut colour = ColourClient {
            client,
            queue,
            state: ColourState {
                version,
                ..ColourState::default()
            },
        };
        // The globals, then what the manager lists.
        colour.roundtrip(compositor);
        colour.roundtrip(compositor);
        assert!(colour.state.manager.is_some(), "no wp_color_manager_v1");
        colour
    }

    /// A roundtrip of the client, then the colour events it read.
    pub fn roundtrip(&mut self, compositor: &mut Compositor) {
        self.client.roundtrip(compositor);
        self.dispatch();
    }

    /// A roundtrip for a request that breaks the protocol: the interface and code of the error
    /// that ended the connection.
    pub fn protocol_error(&mut self, compositor: &mut Compositor) -> (String, u32) {
        match self.client.try_roundtrip(compositor) {
            Err(DispatchError::Backend(WaylandError::Protocol(error))) => {
                (error.object_interface, error.code)
            }
            other => panic!("expected a protocol error, got {other:?}"),
        }
    }

    /// The handle of the colour objects' queue, for a request the helpers do not make.
    pub fn queue_handle(&self) -> QueueHandle<ColourState> {
        self.queue.handle()
    }

    fn dispatch(&mut self) {
        self.queue
            .dispatch_pending(&mut self.state)
            .expect("the colour events are read");
    }

    fn manager(&self) -> &wp_color_manager_v1::WpColorManagerV1 {
        self.state.manager.as_ref().expect("the manager")
    }

    /// Asks output `output`'s description; returns its index.
    pub fn output_description(&mut self, output: usize) -> usize {
        let qh = self.queue.handle();
        let colour_output = self
            .manager()
            .get_output(&self.state.outputs[output], &qh, ());
        let description = colour_output.get_image_description(&qh, ());
        self.push(description)
    }

    /// Makes a colour surface for `surface`.
    pub fn colour_surface(
        &mut self,
        surface: &WlSurface,
    ) -> wp_color_management_surface_v1::WpColorManagementSurfaceV1 {
        let qh = self.queue.handle();
        self.manager().get_surface(surface, &qh, ())
    }

    /// Makes a feedback object for `surface`; returns its index.
    pub fn feedback(&mut self, surface: &WlSurface) -> usize {
        let qh = self.queue.handle();
        let feedback = self.manager().get_surface_feedback(surface, &qh, ());
        self.state.feedbacks.push((feedback, Vec::new()));
        self.state.feedbacks.len() - 1
    }

    /// The identities feedback `index` was told changed, in order.
    pub fn preferred_changes(&self, index: usize) -> &[u64] {
        &self.state.feedbacks[index].1
    }

    /// Asks feedback `index` its surface's preferred description; returns its index.
    pub fn preferred(&mut self, index: usize) -> usize {
        let qh = self.queue.handle();
        let description = self.state.feedbacks[index].0.get_preferred(&qh, ());
        self.push(description)
    }

    /// A parametric creator.
    pub fn params(&mut self) -> Params {
        let qh = self.queue.handle();
        self.manager().create_parametric_creator(&qh, ())
    }

    /// Creates the description of `params`; returns its index.
    pub fn create(&mut self, params: Params) -> usize {
        let qh = self.queue.handle();
        let description = params.create(&qh, ());
        self.push(description)
    }

    /// Asks description `index`'s information.
    pub fn information(&mut self, index: usize) {
        let qh = self.queue.handle();
        self.state.descriptions[index].0.get_information(&qh, index);
    }

    /// The protocol object of description `index`.
    pub fn description(&self, index: usize) -> &wp_image_description_v1::WpImageDescriptionV1 {
        &self.state.descriptions[index].0
    }

    /// What description `index` was told.
    pub fn events(&self, index: usize) -> &DescriptionEvents {
        &self.state.descriptions[index].1
    }

    fn push(&mut self, description: wp_image_description_v1::WpImageDescriptionV1) -> usize {
        // Its index is its user data: the events find it by its place.
        self.state
            .descriptions
            .push((description, DescriptionEvents::default()));
        self.state.descriptions.len() - 1
    }
}

/// Sends request `opcode` of `proxy` with one `uint`, any value: for enum values no typed
/// request can carry.
pub fn send_raw_uint(proxy: &impl Proxy, opcode: u16, value: u32) {
    let backend = proxy.backend().upgrade().expect("connected");
    let message = Message {
        sender_id: proxy.id(),
        opcode,
        args: std::iter::once(Argument::Uint(value)).collect(),
    };
    backend
        .send_request(message, None, None)
        .expect("the request is sent");
}

/// `set_image_description` with any intent, which the typed request cannot send.
pub fn set_raw_description(
    surface: &ColourSurface,
    description: &wp_image_description_v1::WpImageDescriptionV1,
    intent: u32,
) {
    let backend = surface.backend().upgrade().expect("connected");
    let message = Message {
        sender_id: surface.id(),
        opcode: 1,
        args: [Argument::Object(description.id()), Argument::Uint(intent)]
            .into_iter()
            .collect(),
    };
    backend
        .send_request(message, None, None)
        .expect("the request is sent");
}

impl Dispatch<wl_registry::WlRegistry, ()> for ColourState {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &wayland_client::Connection,
        qh: &QueueHandle<Self>,
    ) {
        let wl_registry::Event::Global {
            name,
            interface,
            version,
        } = event
        else {
            return;
        };
        match interface.as_str() {
            "wl_output" => state
                .outputs
                .push(registry.bind(name, version.min(4), qh, ())),
            "wp_color_manager_v1" => {
                assert!(version >= state.version, "offered at version {version}");
                state.manager = Some(registry.bind(name, state.version, qh, ()));
            }
            _ => {}
        }
    }
}

impl Dispatch<wl_output::WlOutput, ()> for ColourState {
    fn event(
        _: &mut Self,
        _: &wl_output::WlOutput,
        _: wl_output::Event,
        _: &(),
        _: &wayland_client::Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

/// An enum's value as the wire carries it.
fn raw<T: Into<u32>>(value: WEnum<T>) -> u32 {
    match value {
        WEnum::Value(value) => value.into(),
        WEnum::Unknown(value) => value,
    }
}

impl Dispatch<wp_color_manager_v1::WpColorManagerV1, ()> for ColourState {
    fn event(
        state: &mut Self,
        _: &wp_color_manager_v1::WpColorManagerV1,
        event: wp_color_manager_v1::Event,
        _: &(),
        _: &wayland_client::Connection,
        _: &QueueHandle<Self>,
    ) {
        use wp_color_manager_v1::Event;
        let listed = &mut state.listed;
        match event {
            Event::SupportedIntent { render_intent } => listed.intents.push(raw(render_intent)),
            Event::SupportedFeature { feature } => listed.features.push(raw(feature)),
            Event::SupportedTfNamed { tf } => listed.transfer_functions.push(raw(tf)),
            Event::SupportedPrimariesNamed { primaries } => listed.primaries.push(raw(primaries)),
            Event::Done => listed.done = true,
            _ => {}
        }
    }
}

impl Dispatch<wp_color_management_output_v1::WpColorManagementOutputV1, ()> for ColourState {
    fn event(
        _: &mut Self,
        _: &wp_color_management_output_v1::WpColorManagementOutputV1,
        _: wp_color_management_output_v1::Event,
        _: &(),
        _: &wayland_client::Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<wp_color_management_surface_v1::WpColorManagementSurfaceV1, ()> for ColourState {
    fn event(
        _: &mut Self,
        _: &wp_color_management_surface_v1::WpColorManagementSurfaceV1,
        _: wp_color_management_surface_v1::Event,
        _: &(),
        _: &wayland_client::Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<wp_color_management_surface_feedback_v1::WpColorManagementSurfaceFeedbackV1, ()>
    for ColourState
{
    fn event(
        state: &mut Self,
        feedback: &wp_color_management_surface_feedback_v1::WpColorManagementSurfaceFeedbackV1,
        event: wp_color_management_surface_feedback_v1::Event,
        _: &(),
        _: &wayland_client::Connection,
        _: &QueueHandle<Self>,
    ) {
        use wp_color_management_surface_feedback_v1::Event;
        let identity = match event {
            Event::PreferredChanged { identity } => u64::from(identity),
            Event::PreferredChanged2 {
                identity_hi,
                identity_lo,
            } => (u64::from(identity_hi) << 32) | u64::from(identity_lo),
            _ => return,
        };
        if let Some((_, changes)) = state.feedbacks.iter_mut().find(|(f, _)| f == feedback) {
            changes.push(identity);
        }
    }
}

impl Dispatch<wp_image_description_creator_params_v1::WpImageDescriptionCreatorParamsV1, ()>
    for ColourState
{
    fn event(
        _: &mut Self,
        _: &Params,
        _: wp_image_description_creator_params_v1::Event,
        _: &(),
        _: &wayland_client::Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<wp_image_description_creator_icc_v1::WpImageDescriptionCreatorIccV1, ()>
    for ColourState
{
    fn event(
        _: &mut Self,
        _: &wp_image_description_creator_icc_v1::WpImageDescriptionCreatorIccV1,
        _: wp_image_description_creator_icc_v1::Event,
        _: &(),
        _: &wayland_client::Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<wp_image_description_v1::WpImageDescriptionV1, ()> for ColourState {
    fn event(
        state: &mut Self,
        description: &wp_image_description_v1::WpImageDescriptionV1,
        event: wp_image_description_v1::Event,
        _: &(),
        _: &wayland_client::Connection,
        _: &QueueHandle<Self>,
    ) {
        use wp_image_description_v1::Event;
        let Some((_, events)) = state
            .descriptions
            .iter_mut()
            .find(|(d, _)| d == description)
        else {
            return;
        };
        match event {
            Event::Ready { identity } => events.ready = Some(u64::from(identity)),
            Event::Ready2 {
                identity_hi,
                identity_lo,
            } => {
                events.ready = Some((u64::from(identity_hi) << 32) | u64::from(identity_lo));
                events.ready2 = true;
            }
            Event::Failed { cause, msg } => events.failed = Some((raw(cause), msg)),
            _ => {}
        }
    }
}

impl Dispatch<wp_image_description_info_v1::WpImageDescriptionInfoV1, usize> for ColourState {
    fn event(
        state: &mut Self,
        _: &wp_image_description_info_v1::WpImageDescriptionInfoV1,
        event: wp_image_description_info_v1::Event,
        index: &usize,
        _: &wayland_client::Connection,
        _: &QueueHandle<Self>,
    ) {
        use wp_image_description_info_v1::Event;
        let info = state.descriptions[*index]
            .1
            .information
            .get_or_insert_with(Information::default);
        match event {
            Event::Primaries {
                r_x,
                r_y,
                g_x,
                g_y,
                b_x,
                b_y,
                w_x,
                w_y,
            } => info.primaries = Some([r_x, r_y, g_x, g_y, b_x, b_y, w_x, w_y]),
            Event::PrimariesNamed { primaries } => info.primaries_named = Some(raw(primaries)),
            Event::TfNamed { tf } => info.tf_named = Some(raw(tf)),
            Event::Luminances {
                min_lum,
                max_lum,
                reference_lum,
            } => info.luminances = Some((min_lum, max_lum, reference_lum)),
            Event::TargetPrimaries {
                r_x,
                r_y,
                g_x,
                g_y,
                b_x,
                b_y,
                w_x,
                w_y,
            } => info.target_primaries = Some([r_x, r_y, g_x, g_y, b_x, b_y, w_x, w_y]),
            Event::TargetLuminance { min_lum, max_lum } => {
                info.target_luminance = Some((min_lum, max_lum));
            }
            Event::TargetMaxCll { max_cll } => info.target_max_cll = Some(max_cll),
            Event::TargetMaxFall { max_fall } => info.target_max_fall = Some(max_fall),
            Event::Done => info.done = true,
            _ => {}
        }
    }
}
