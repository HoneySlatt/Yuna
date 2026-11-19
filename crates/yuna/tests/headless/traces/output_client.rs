//! A client of `wl_output` and `xdg-output` for the traces: it binds every output's `wl_output`
//! and makes its `zxdg_output_v1`, as each is announced, and lets go of both when the output's
//! global is removed. It keeps nothing of what they say: the trace holds it.
//!
//! An [`OutputsClient`] is a test [`Client`] with these objects on an event queue of their own,
//! over the same connection, as `capture_client.rs` does.

use std::path::Path;

use wayland_client::protocol::{wl_output, wl_registry};
use wayland_client::{Connection, Dispatch, EventQueue, QueueHandle, delegate_noop};
use wayland_protocols::xdg::xdg_output::zv1::client::{zxdg_output_manager_v1, zxdg_output_v1};
use yuna::Compositor;

use crate::client::Client;

/// A test client of the outputs.
pub struct OutputsClient {
    pub client: Client,
    queue: EventQueue<OutputsState>,
    state: OutputsState,
}

/// The client's outputs.
#[derive(Default)]
struct OutputsState {
    manager: Option<zxdg_output_manager_v1::ZxdgOutputManagerV1>,
    /// Each output bound, by its registry name, with its `xdg-output` once made.
    outputs: Vec<(
        u32,
        wl_output::WlOutput,
        Option<zxdg_output_v1::ZxdgOutputV1>,
    )>,
}

impl OutputsClient {
    /// Connects a client of the owner's seat, and binds its outputs.
    pub fn connect(compositor: &mut Compositor) -> OutputsClient {
        let mut outputs = OutputsClient::bind(Client::connect(compositor));
        // The globals, then the outputs' events.
        outputs.roundtrip(compositor);
        outputs.roundtrip(compositor);
        outputs
    }

    /// Connects to the Yuna process whose socket is at `path`, and binds its outputs.
    pub fn connect_to_process(path: &Path) -> OutputsClient {
        let mut outputs = OutputsClient::bind(Client::connect_to_process(path));
        outputs.roundtrip_with_process();
        outputs.roundtrip_with_process();
        outputs
    }

    fn bind(client: Client) -> OutputsClient {
        let queue = client.connection().new_event_queue();
        client
            .connection()
            .display()
            .get_registry(&queue.handle(), ());
        OutputsClient {
            client,
            queue,
            state: OutputsState::default(),
        }
    }

    /// A roundtrip of the client, then the outputs' events it read.
    pub fn roundtrip(&mut self, compositor: &mut Compositor) {
        self.client.roundtrip(compositor);
        self.dispatch();
    }

    /// As [`OutputsClient::roundtrip`], with Yuna run as a process.
    pub fn roundtrip_with_process(&mut self) {
        self.client.roundtrip_with_process();
        self.dispatch();
    }

    fn dispatch(&mut self) {
        self.queue
            .dispatch_pending(&mut self.state)
            .expect("the outputs' events are read");
    }
}

impl OutputsState {
    /// Makes the `xdg-output` of every output that has none, once the manager is bound.
    fn make_xdg_outputs(&mut self, qh: &QueueHandle<Self>) {
        let Some(manager) = &self.manager else {
            return;
        };
        for (_, output, xdg_output) in &mut self.outputs {
            if xdg_output.is_none() {
                *xdg_output = Some(manager.get_xdg_output(output, qh, ()));
            }
        }
    }
}

impl Dispatch<wl_registry::WlRegistry, ()> for OutputsState {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        match event {
            wl_registry::Event::Global {
                name,
                interface,
                version,
            } => match interface.as_str() {
                "wl_output" => {
                    let output = registry.bind(name, version.min(4), qh, ());
                    state.outputs.push((name, output, None));
                    state.make_xdg_outputs(qh);
                }
                "zxdg_output_manager_v1" => {
                    state.manager = Some(registry.bind(name, version.min(3), qh, ()));
                    state.make_xdg_outputs(qh);
                }
                _ => {}
            },
            wl_registry::Event::GlobalRemove { name } => {
                if let Some(index) = state.outputs.iter().position(|(bound, ..)| *bound == name) {
                    let (_, output, xdg_output) = state.outputs.remove(index);
                    if let Some(xdg_output) = xdg_output {
                        xdg_output.destroy();
                    }
                    output.release();
                }
            }
            _ => {}
        }
    }
}

delegate_noop!(OutputsState: ignore wl_output::WlOutput);
delegate_noop!(OutputsState: ignore zxdg_output_manager_v1::ZxdgOutputManagerV1);
delegate_noop!(OutputsState: ignore zxdg_output_v1::ZxdgOutputV1);
