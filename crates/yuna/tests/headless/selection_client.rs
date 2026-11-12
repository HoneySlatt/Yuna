//! The selection protocols' client side, for tests: the clipboard (`wl_data_device`), the primary
//! selection and `ext-data-control-v1`, with the seat's keyboard, whose `enter` serials a
//! selection request names.
//!
//! A [`SelectionClient`] is a test [`Client`] with these protocols on an event queue of their
//! own, over the same connection, so that `client.rs` stays about windows: the windows and the
//! selections are one Wayland client, which is what the keyboard focus looks at. It records
//! every offer and every selection event in order, and what each of its sources is asked.

use std::fs::File;
use std::io::{ErrorKind, Read, Write};
use std::os::fd::{AsFd, OwnedFd};

use smithay::reexports::rustix::fs::{OFlags, fcntl_setfl};
use smithay::reexports::rustix::pipe::{PipeFlags, pipe_with};
use wayland_client::backend::ObjectId;
use wayland_client::protocol::{
    wl_data_device, wl_data_device_manager, wl_data_offer, wl_data_source, wl_keyboard,
    wl_registry, wl_seat, wl_surface,
};
use wayland_client::{
    Connection, Dispatch, DispatchError, EventQueue, Proxy, QueueHandle, delegate_noop,
    event_created_child,
};
use wayland_protocols::ext::data_control::v1::client::{
    ext_data_control_device_v1, ext_data_control_manager_v1, ext_data_control_offer_v1,
    ext_data_control_source_v1,
};
use wayland_protocols::wp::primary_selection::zv1::client::{
    zwp_primary_selection_device_manager_v1, zwp_primary_selection_device_v1,
    zwp_primary_selection_offer_v1, zwp_primary_selection_source_v1,
};
use yuna::Compositor;

use crate::client::Client;

/// What a device told its client the selections hold, in order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum SelectionEvent {
    /// `wl_data_device.selection`: the clipboard holds the offer of this index, or nothing.
    Clipboard(Option<usize>),
    /// `zwp_primary_selection_device_v1.selection`.
    Primary(Option<usize>),
    /// `ext_data_control_device_v1.selection`.
    ControlClipboard(Option<usize>),
    /// `ext_data_control_device_v1.primary_selection`.
    ControlPrimary(Option<usize>),
    /// A drag-and-drop event of `wl_data_device`: `enter`, `leave`, `motion` or `drop`.
    Drag,
}

/// An offer the client was given, and the MIME types it offers.
pub struct Offer {
    proxy: OfferProxy,
    pub mime_types: Vec<String>,
}

enum OfferProxy {
    Clipboard(wl_data_offer::WlDataOffer),
    Primary(zwp_primary_selection_offer_v1::ZwpPrimarySelectionOfferV1),
    Control(ext_data_control_offer_v1::ExtDataControlOfferV1),
}

impl OfferProxy {
    fn id(&self) -> ObjectId {
        match self {
            OfferProxy::Clipboard(offer) => offer.id(),
            OfferProxy::Primary(offer) => offer.id(),
            OfferProxy::Control(offer) => offer.id(),
        }
    }
}

/// A source the client made, and what it was asked.
pub struct Source {
    proxy: SourceProxy,
    /// The MIME type of each `send` event, in order.
    pub asked: Vec<String>,
    /// The file descriptors of the `send` events not answered yet.
    waiting: Vec<OwnedFd>,
    /// Whether it got `cancelled`: another selection replaced it.
    pub cancelled: bool,
    /// Drag-and-drop events: `target`, `action`, `dnd_drop_performed`, `dnd_finished`.
    pub drag_events: u32,
}

enum SourceProxy {
    Clipboard(wl_data_source::WlDataSource),
    Primary(zwp_primary_selection_source_v1::ZwpPrimarySelectionSourceV1),
    Control(ext_data_control_source_v1::ExtDataControlSourceV1),
}

/// The read end of a pipe given with a `receive` request. Reading it never waits.
pub struct Pipe(File);

impl Pipe {
    /// Everything written into the pipe, once every writer has closed it. Fails, rather than
    /// waits, while one still holds it open.
    pub fn read_all(mut self) -> Vec<u8> {
        let mut data = Vec::new();
        match self.0.read_to_end(&mut data) {
            Ok(_) => data,
            Err(error) if error.kind() == ErrorKind::WouldBlock => {
                panic!("a writer still holds the pipe open, after {data:?}")
            }
            Err(error) => panic!("the pipe cannot be read: {error}"),
        }
    }
}

/// A test client with the selection protocols.
pub struct SelectionClient {
    /// Its windows' side.
    pub client: Client,
    queue: EventQueue<SelectionState>,
    pub state: SelectionState,
}

/// What the client's selection objects hold and received.
#[derive(Default)]
pub struct SelectionState {
    seat: Option<wl_seat::WlSeat>,
    data_device_manager: Option<wl_data_device_manager::WlDataDeviceManager>,
    primary_manager:
        Option<zwp_primary_selection_device_manager_v1::ZwpPrimarySelectionDeviceManagerV1>,
    control_manager: Option<ext_data_control_manager_v1::ExtDataControlManagerV1>,
    _keyboard: Option<wl_keyboard::WlKeyboard>,
    data_device: Option<wl_data_device::WlDataDevice>,
    primary_device: Option<zwp_primary_selection_device_v1::ZwpPrimarySelectionDeviceV1>,
    control_device: Option<ext_data_control_device_v1::ExtDataControlDeviceV1>,
    /// The serial of each `wl_keyboard.enter`, in order.
    pub enters: Vec<u32>,
    /// Every offer the client was given, in order; events name them by index.
    pub offers: Vec<Offer>,
    pub events: Vec<SelectionEvent>,
    pub sources: Vec<Source>,
}

impl SelectionClient {
    /// A client with a keyboard, a data device and a primary selection device, as an
    /// application has; its windows open through [`SelectionClient::client`].
    pub fn connect(compositor: &mut Compositor) -> SelectionClient {
        let mut selection_client = SelectionClient::bind(compositor);
        let qh = selection_client.queue.handle();
        let state = &mut selection_client.state;
        let seat = state.seat.clone().expect("wl_seat");
        state._keyboard = Some(seat.get_keyboard(&qh, ()));
        state.data_device = Some(
            state
                .data_device_manager
                .as_ref()
                .expect("wl_data_device_manager")
                .get_data_device(&seat, &qh, ()),
        );
        state.primary_device = Some(
            state
                .primary_manager
                .as_ref()
                .expect("zwp_primary_selection_device_manager_v1")
                .get_device(&seat, &qh, ()),
        );
        selection_client.roundtrip(compositor);
        selection_client
    }

    /// A client with an `ext_data_control_device_v1` alone, as a clipboard manager has.
    pub fn connect_data_control(compositor: &mut Compositor) -> SelectionClient {
        let mut selection_client = SelectionClient::bind(compositor);
        let qh = selection_client.queue.handle();
        let state = &mut selection_client.state;
        let seat = state.seat.clone().expect("wl_seat");
        state.control_device = Some(
            state
                .control_manager
                .as_ref()
                .expect("ext_data_control_manager_v1")
                .get_data_device(&seat, &qh, ()),
        );
        selection_client.roundtrip(compositor);
        selection_client
    }

    /// Connects a client and binds the seat and the selections' managers on its queue.
    fn bind(compositor: &mut Compositor) -> SelectionClient {
        let client = Client::connect(compositor);
        let queue = client.connection().new_event_queue();
        client
            .connection()
            .display()
            .get_registry(&queue.handle(), ());
        let mut selection_client = SelectionClient {
            client,
            queue,
            state: SelectionState::default(),
        };
        selection_client.roundtrip(compositor);
        selection_client
    }

    /// A roundtrip of the client, then the selection events it read.
    pub fn roundtrip(&mut self, compositor: &mut Compositor) {
        if let Err(error) = self.try_roundtrip(compositor) {
            panic!("the client's connection failed: {error}");
        }
    }

    /// A roundtrip for a client that may break the protocol: the error that ended its
    /// connection rather than a panic, as [`Client::try_roundtrip`].
    pub fn try_roundtrip(&mut self, compositor: &mut Compositor) -> Result<(), DispatchError> {
        // The client's roundtrip reads every event sent before its answer, this queue's too.
        self.client.try_roundtrip(compositor)?;
        self.queue.dispatch_pending(&mut self.state)?;
        Ok(())
    }

    /// The serial of the last `wl_keyboard.enter`.
    pub fn enter_serial(&self) -> u32 {
        *self.state.enters.last().expect("a keyboard enter")
    }

    pub fn source(&self, index: usize) -> &Source {
        &self.state.sources[index]
    }

    /// Sets the clipboard to a new source offering `mime_types`, naming `serial`.
    pub fn set_clipboard(&mut self, mime_types: &[&str], serial: u32) -> usize {
        let source = self.clipboard_source(mime_types);
        self.state
            .data_device
            .as_ref()
            .expect("wl_data_device")
            .set_selection(Some(&source), serial);
        self.add_source(SourceProxy::Clipboard(source))
    }

    /// Sets the primary selection to a new source offering `mime_types`, naming `serial`.
    pub fn set_primary(&mut self, mime_types: &[&str], serial: u32) -> usize {
        let index = self.state.sources.len();
        let source = self
            .state
            .primary_manager
            .as_ref()
            .expect("zwp_primary_selection_device_manager_v1")
            .create_source(&self.queue.handle(), index);
        for mime_type in mime_types {
            source.offer((*mime_type).to_owned());
        }
        self.state
            .primary_device
            .as_ref()
            .expect("zwp_primary_selection_device_v1")
            .set_selection(Some(&source), serial);
        self.add_source(SourceProxy::Primary(source))
    }

    /// Sets the clipboard through data control to a new source offering `mime_types`.
    pub fn control_set_clipboard(&mut self, mime_types: &[&str]) -> usize {
        let index = self.state.sources.len();
        let source = self
            .state
            .control_manager
            .as_ref()
            .expect("ext_data_control_manager_v1")
            .create_data_source(&self.queue.handle(), index);
        for mime_type in mime_types {
            source.offer((*mime_type).to_owned());
        }
        self.add_source(SourceProxy::Control(source));
        self.control_set_clipboard_source(index);
        index
    }

    /// Sets the clipboard through data control to source `source`. A data-control source serves
    /// once: setting it a second time is a protocol error.
    pub fn control_set_clipboard_source(&mut self, source: usize) {
        let SourceProxy::Control(source) = &self.state.sources[source].proxy else {
            panic!("not a data-control source");
        };
        self.state
            .control_device
            .as_ref()
            .expect("ext_data_control_device_v1")
            .set_selection(Some(source));
    }

    /// Starts a drag of a new source offering `mime_types` from `origin`, naming `serial`.
    pub fn start_drag(
        &mut self,
        mime_types: &[&str],
        origin: &wl_surface::WlSurface,
        serial: u32,
    ) -> usize {
        let source = self.clipboard_source(mime_types);
        self.state
            .data_device
            .as_ref()
            .expect("wl_data_device")
            .start_drag(Some(&source), origin, None, serial);
        self.add_source(SourceProxy::Clipboard(source))
    }

    /// A new `wl_data_source` offering `mime_types`, for the next index of the sources.
    fn clipboard_source(&mut self, mime_types: &[&str]) -> wl_data_source::WlDataSource {
        let index = self.state.sources.len();
        let source = self
            .state
            .data_device_manager
            .as_ref()
            .expect("wl_data_device_manager")
            .create_data_source(&self.queue.handle(), index);
        for mime_type in mime_types {
            source.offer((*mime_type).to_owned());
        }
        source
    }

    fn add_source(&mut self, proxy: SourceProxy) -> usize {
        self.state.sources.push(Source {
            proxy,
            asked: Vec::new(),
            waiting: Vec::new(),
            cancelled: false,
            drag_events: 0,
        });
        self.state.sources.len() - 1
    }

    /// Asks offer `offer` for its data as `mime_type`, into a new pipe whose read end it
    /// returns. The write end goes with the request; the test's own copy is closed at once.
    pub fn receive(&mut self, offer: usize, mime_type: &str) -> Pipe {
        let (read, write) = pipe_with(PipeFlags::CLOEXEC).expect("a pipe");
        fcntl_setfl(&read, OFlags::NONBLOCK).expect("a non-blocking read end");
        let mime_type = mime_type.to_owned();
        match &self.state.offers[offer].proxy {
            OfferProxy::Clipboard(offer) => offer.receive(mime_type, write.as_fd()),
            OfferProxy::Primary(offer) => offer.receive(mime_type, write.as_fd()),
            OfferProxy::Control(offer) => offer.receive(mime_type, write.as_fd()),
        }
        Pipe(File::from(read))
    }

    /// Writes `data` into every pipe source `source` was sent and not answered yet, then closes
    /// them, as a source's client does.
    pub fn answer(&mut self, source: usize, data: &[u8]) {
        for fd in self.state.sources[source].waiting.drain(..) {
            File::from(fd)
                .write_all(data)
                .expect("the pipe takes the data");
        }
    }

    pub fn destroy_source(&mut self, source: usize) {
        match &self.state.sources[source].proxy {
            SourceProxy::Clipboard(source) => source.destroy(),
            SourceProxy::Primary(source) => source.destroy(),
            SourceProxy::Control(source) => source.destroy(),
        }
    }

    pub fn destroy_offer(&mut self, offer: usize) {
        match &self.state.offers[offer].proxy {
            OfferProxy::Clipboard(offer) => offer.destroy(),
            OfferProxy::Primary(offer) => offer.destroy(),
            OfferProxy::Control(offer) => offer.destroy(),
        }
    }
}

impl SelectionState {
    fn add_offer(&mut self, proxy: OfferProxy) {
        self.offers.push(Offer {
            proxy,
            mime_types: Vec::new(),
        });
    }

    /// The index of the offer `id`, which the client was given.
    fn offer_index(&self, id: &ObjectId) -> usize {
        self.offers
            .iter()
            .position(|offer| offer.proxy.id() == *id)
            .expect("an offer the client was given")
    }

    fn add_mime_type(&mut self, offer: &ObjectId, mime_type: String) {
        let index = self.offer_index(offer);
        self.offers[index].mime_types.push(mime_type);
    }

    fn sent(&mut self, source: usize, mime_type: String, fd: OwnedFd) {
        let source = &mut self.sources[source];
        source.asked.push(mime_type);
        source.waiting.push(fd);
    }
}

impl Dispatch<wl_registry::WlRegistry, ()> for SelectionState {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
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
            "wl_seat" => state.seat = Some(registry.bind(name, version.min(9), qh, ())),
            "wl_data_device_manager" => {
                state.data_device_manager = Some(registry.bind(name, version.min(3), qh, ()));
            }
            "zwp_primary_selection_device_manager_v1" => {
                state.primary_manager = Some(registry.bind(name, 1, qh, ()));
            }
            "ext_data_control_manager_v1" => {
                state.control_manager = Some(registry.bind(name, 1, qh, ()));
            }
            _ => {}
        }
    }
}

impl Dispatch<wl_keyboard::WlKeyboard, ()> for SelectionState {
    fn event(
        state: &mut Self,
        _: &wl_keyboard::WlKeyboard,
        event: wl_keyboard::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        // The keymap's file descriptor, in its own event, is closed as the event drops.
        if let wl_keyboard::Event::Enter { serial, .. } = event {
            state.enters.push(serial);
        }
    }
}

impl Dispatch<wl_data_device::WlDataDevice, ()> for SelectionState {
    fn event(
        state: &mut Self,
        _: &wl_data_device::WlDataDevice,
        event: wl_data_device::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            wl_data_device::Event::DataOffer { id } => state.add_offer(OfferProxy::Clipboard(id)),
            wl_data_device::Event::Selection { id } => {
                let offer = id.map(|offer| state.offer_index(&offer.id()));
                state.events.push(SelectionEvent::Clipboard(offer));
            }
            wl_data_device::Event::Enter { .. }
            | wl_data_device::Event::Leave
            | wl_data_device::Event::Motion { .. }
            | wl_data_device::Event::Drop => state.events.push(SelectionEvent::Drag),
            _ => {}
        }
    }

    event_created_child!(SelectionState, wl_data_device::WlDataDevice, [
        wl_data_device::EVT_DATA_OFFER_OPCODE => (wl_data_offer::WlDataOffer, ()),
    ]);
}

impl Dispatch<wl_data_offer::WlDataOffer, ()> for SelectionState {
    fn event(
        state: &mut Self,
        offer: &wl_data_offer::WlDataOffer,
        event: wl_data_offer::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_data_offer::Event::Offer { mime_type } = event {
            state.add_mime_type(&offer.id(), mime_type);
        }
    }
}

impl Dispatch<wl_data_source::WlDataSource, usize> for SelectionState {
    fn event(
        state: &mut Self,
        _: &wl_data_source::WlDataSource,
        event: wl_data_source::Event,
        index: &usize,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            wl_data_source::Event::Send { mime_type, fd } => state.sent(*index, mime_type, fd),
            wl_data_source::Event::Cancelled => state.sources[*index].cancelled = true,
            _ => state.sources[*index].drag_events += 1,
        }
    }
}

impl Dispatch<zwp_primary_selection_device_v1::ZwpPrimarySelectionDeviceV1, ()> for SelectionState {
    fn event(
        state: &mut Self,
        _: &zwp_primary_selection_device_v1::ZwpPrimarySelectionDeviceV1,
        event: zwp_primary_selection_device_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            zwp_primary_selection_device_v1::Event::DataOffer { offer } => {
                state.add_offer(OfferProxy::Primary(offer));
            }
            zwp_primary_selection_device_v1::Event::Selection { id } => {
                let offer = id.map(|offer| state.offer_index(&offer.id()));
                state.events.push(SelectionEvent::Primary(offer));
            }
            _ => {}
        }
    }

    event_created_child!(
        SelectionState,
        zwp_primary_selection_device_v1::ZwpPrimarySelectionDeviceV1,
        [
            zwp_primary_selection_device_v1::EVT_DATA_OFFER_OPCODE =>
                (zwp_primary_selection_offer_v1::ZwpPrimarySelectionOfferV1, ()),
        ]
    );
}

impl Dispatch<zwp_primary_selection_offer_v1::ZwpPrimarySelectionOfferV1, ()> for SelectionState {
    fn event(
        state: &mut Self,
        offer: &zwp_primary_selection_offer_v1::ZwpPrimarySelectionOfferV1,
        event: zwp_primary_selection_offer_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let zwp_primary_selection_offer_v1::Event::Offer { mime_type } = event {
            state.add_mime_type(&offer.id(), mime_type);
        }
    }
}

impl Dispatch<zwp_primary_selection_source_v1::ZwpPrimarySelectionSourceV1, usize>
    for SelectionState
{
    fn event(
        state: &mut Self,
        _: &zwp_primary_selection_source_v1::ZwpPrimarySelectionSourceV1,
        event: zwp_primary_selection_source_v1::Event,
        index: &usize,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            zwp_primary_selection_source_v1::Event::Send { mime_type, fd } => {
                state.sent(*index, mime_type, fd);
            }
            zwp_primary_selection_source_v1::Event::Cancelled => {
                state.sources[*index].cancelled = true;
            }
            _ => {}
        }
    }
}

impl Dispatch<ext_data_control_device_v1::ExtDataControlDeviceV1, ()> for SelectionState {
    fn event(
        state: &mut Self,
        _: &ext_data_control_device_v1::ExtDataControlDeviceV1,
        event: ext_data_control_device_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            ext_data_control_device_v1::Event::DataOffer { id } => {
                state.add_offer(OfferProxy::Control(id));
            }
            ext_data_control_device_v1::Event::Selection { id } => {
                let offer = id.map(|offer| state.offer_index(&offer.id()));
                state.events.push(SelectionEvent::ControlClipboard(offer));
            }
            ext_data_control_device_v1::Event::PrimarySelection { id } => {
                let offer = id.map(|offer| state.offer_index(&offer.id()));
                state.events.push(SelectionEvent::ControlPrimary(offer));
            }
            _ => {}
        }
    }

    event_created_child!(SelectionState, ext_data_control_device_v1::ExtDataControlDeviceV1, [
        ext_data_control_device_v1::EVT_DATA_OFFER_OPCODE =>
            (ext_data_control_offer_v1::ExtDataControlOfferV1, ()),
    ]);
}

impl Dispatch<ext_data_control_offer_v1::ExtDataControlOfferV1, ()> for SelectionState {
    fn event(
        state: &mut Self,
        offer: &ext_data_control_offer_v1::ExtDataControlOfferV1,
        event: ext_data_control_offer_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let ext_data_control_offer_v1::Event::Offer { mime_type } = event {
            state.add_mime_type(&offer.id(), mime_type);
        }
    }
}

impl Dispatch<ext_data_control_source_v1::ExtDataControlSourceV1, usize> for SelectionState {
    fn event(
        state: &mut Self,
        _: &ext_data_control_source_v1::ExtDataControlSourceV1,
        event: ext_data_control_source_v1::Event,
        index: &usize,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            ext_data_control_source_v1::Event::Send { mime_type, fd } => {
                state.sent(*index, mime_type, fd);
            }
            ext_data_control_source_v1::Event::Cancelled => {
                state.sources[*index].cancelled = true;
            }
            _ => {}
        }
    }
}

delegate_noop!(SelectionState: ignore wl_seat::WlSeat);
delegate_noop!(SelectionState: ignore wl_data_device_manager::WlDataDeviceManager);
delegate_noop!(SelectionState: ignore zwp_primary_selection_device_manager_v1::ZwpPrimarySelectionDeviceManagerV1);
delegate_noop!(SelectionState: ignore ext_data_control_manager_v1::ExtDataControlManagerV1);
