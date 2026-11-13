//! The test client's popups: created on a window or on another popup through an
//! `xdg_positioner`, and what each received: configures, `repositioned`, `popup_done`, frame
//! callbacks, presentation feedback and the scale it is told.

use wayland_client::protocol::{wl_callback, wl_seat, wl_surface};
use wayland_client::{Connection, Dispatch, QueueHandle};
use wayland_protocols::wp::fractional_scale::v1::client::wp_fractional_scale_v1;
use wayland_protocols::wp::presentation_time::client::wp_presentation_feedback;
use wayland_protocols::xdg::shell::client::{xdg_popup, xdg_positioner, xdg_surface};
use yuna::Compositor;

use super::{Client, ClientState, Feedback};

pub use wayland_protocols::xdg::shell::client::xdg_positioner::{
    Anchor, ConstraintAdjustment, Gravity,
};

/// One `xdg_surface.configure` of a popup, with the `xdg_popup.configure` before it: where the
/// popup goes, relative to its parent's window geometry.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PopupConfigure {
    pub serial: u32,
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

/// What a popup's positioner says.
#[derive(Debug, Clone, Copy)]
pub struct Placement {
    pub size: (i32, i32),
    /// x, y, width, height, in the parent's window geometry.
    pub anchor_rect: (i32, i32, i32, i32),
    pub anchor: Anchor,
    pub gravity: Gravity,
    pub adjustment: ConstraintAdjustment,
    /// Added to the place the rules give.
    pub offset: (i32, i32),
}

impl Placement {
    /// A popup of `size` hanging from the bottom-right corner of `anchor_rect`, as a menu under a
    /// button, without constraint adjustments.
    pub fn below_right(size: (i32, i32), anchor_rect: (i32, i32, i32, i32)) -> Placement {
        Placement {
            size,
            anchor_rect,
            anchor: Anchor::BottomRight,
            gravity: Gravity::BottomRight,
            adjustment: ConstraintAdjustment::empty(),
            offset: (0, 0),
        }
    }
}

/// What a popup's parent is.
#[derive(Debug, Clone, Copy)]
pub enum Parent {
    Window(usize),
    Popup(usize),
    /// None given: the popup must get one before its first commit, else that commit is its
    /// client's protocol error.
    Unset,
}

/// A popup and what it received.
pub struct Popup {
    pub surface: wl_surface::WlSurface,
    xdg_surface: xdg_surface::XdgSurface,
    popup: xdg_popup::XdgPopup,
    /// The popup configure waiting for its `xdg_surface.configure`.
    pending: PopupConfigure,
    pub configures: Vec<PopupConfigure>,
    /// The serial it last acknowledged.
    pub acked: Option<u32>,
    pub repositioned: Vec<u32>,
    /// Whether it got `popup_done`.
    pub done: bool,
    /// The time each frame callback carried, in milliseconds.
    pub frames: Vec<u32>,
    pub feedback: Vec<Feedback>,
    /// Its `wp_fractional_scale_v1`, once made.
    pub fractional_scale: Option<wp_fractional_scale_v1::WpFractionalScaleV1>,
    /// Each `wp_fractional_scale_v1.preferred_scale`, in 120ths.
    pub preferred_scales: Vec<u32>,
    /// Each `wl_surface.preferred_buffer_scale`.
    pub preferred_buffer_scales: Vec<i32>,
}

impl Popup {
    fn new(
        surface: wl_surface::WlSurface,
        xdg_surface: xdg_surface::XdgSurface,
        popup: xdg_popup::XdgPopup,
    ) -> Popup {
        Popup {
            surface,
            xdg_surface,
            popup,
            pending: PopupConfigure::default(),
            configures: Vec::new(),
            acked: None,
            repositioned: Vec::new(),
            done: false,
            frames: Vec::new(),
            feedback: Vec::new(),
            fractional_scale: None,
            preferred_scales: Vec::new(),
            preferred_buffer_scales: Vec::new(),
        }
    }
}

/// Which popup an object's events are for.
#[derive(Debug, Clone, Copy)]
pub struct PopupIndex(usize);

impl Client {
    pub fn popup(&self, index: usize) -> &Popup {
        &self.state.popups[index]
    }

    /// How many popups the client created.
    pub fn popup_count(&self) -> usize {
        self.state.popups.len()
    }

    /// Creates a popup on `parent`, placed as `placement` says, before its initial commit.
    pub fn create_popup(&mut self, parent: Parent, placement: Placement) -> usize {
        let qh = self.queue.handle();
        let index = self.state.popups.len();
        let positioner = self.positioner(placement);
        let state = &self.state;
        let wm_base = state.wm_base.as_ref().expect("xdg_wm_base");
        let surface = state
            .compositor
            .as_ref()
            .expect("wl_compositor")
            .create_surface(&qh, PopupIndex(index));
        let xdg_surface = wm_base.get_xdg_surface(&surface, &qh, PopupIndex(index));
        let popup = xdg_surface.get_popup(
            self.parent_xdg_surface(parent),
            &positioner,
            &qh,
            PopupIndex(index),
        );
        positioner.destroy();
        self.state
            .popups
            .push(Popup::new(surface, xdg_surface, popup));
        index
    }

    /// Creates two popups, each the other's parent: a cycle xdg-shell forbids. Each `xdg_surface`
    /// is made the other's parent while it has no role yet.
    pub fn create_popup_cycle(&mut self, placement: Placement) -> (usize, usize) {
        let qh = self.queue.handle();
        let (a, b) = (self.state.popups.len(), self.state.popups.len() + 1);
        let positioner = self.positioner(placement);
        let state = &self.state;
        let compositor = state.compositor.as_ref().expect("wl_compositor");
        let wm_base = state.wm_base.as_ref().expect("xdg_wm_base");
        let surface_a = compositor.create_surface(&qh, PopupIndex(a));
        let surface_b = compositor.create_surface(&qh, PopupIndex(b));
        let xdg_a = wm_base.get_xdg_surface(&surface_a, &qh, PopupIndex(a));
        let xdg_b = wm_base.get_xdg_surface(&surface_b, &qh, PopupIndex(b));
        let popup_b = xdg_b.get_popup(Some(&xdg_a), &positioner, &qh, PopupIndex(b));
        let popup_a = xdg_a.get_popup(Some(&xdg_b), &positioner, &qh, PopupIndex(a));
        positioner.destroy();
        for (surface, xdg_surface, popup) in
            [(surface_a, xdg_a, popup_a), (surface_b, xdg_b, popup_b)]
        {
            self.state
                .popups
                .push(Popup::new(surface, xdg_surface, popup));
        }
        (a, b)
    }

    /// Creates a popup, makes its initial commit, and lets the compositor answer.
    pub fn open_popup(
        &mut self,
        compositor: &mut Compositor,
        parent: Parent,
        placement: Placement,
    ) -> usize {
        let popup = self.create_popup(parent, placement);
        self.commit_popup(popup);
        self.roundtrip(compositor);
        popup
    }

    /// Opens a popup, acknowledges its configure and commits a buffer of its size, of one colour,
    /// with a frame callback and presentation feedback.
    pub fn map_popup(
        &mut self,
        compositor: &mut Compositor,
        parent: Parent,
        placement: Placement,
        argb: u32,
    ) -> usize {
        let popup = self.open_popup(compositor, parent, placement);
        self.ack_popup(popup);
        self.attach_popup(popup, placement.size.0, placement.size.1, argb);
        self.request_popup_frame(popup);
        self.request_popup_feedback(popup);
        self.commit_popup(popup);
        self.roundtrip(compositor);
        popup
    }

    /// Acknowledges the popup's last configure.
    pub fn ack_popup(&mut self, popup: usize) {
        let popup = &mut self.state.popups[popup];
        let serial = popup.configures.last().expect("a configure").serial;
        popup.xdg_surface.ack_configure(serial);
        popup.acked = Some(serial);
    }

    /// Attaches a new buffer of `width` × `height` pixels of one colour, `0xAARRGGBB`.
    pub fn attach_popup(&mut self, popup: usize, width: i32, height: i32, argb: u32) {
        let pixels = vec![argb; super::pixel_count(width, height)];
        let buffer = self.buffer(width, height, &pixels);
        let surface = &self.state.popups[popup].surface;
        surface.attach(Some(&buffer), 0, 0);
        surface.damage_buffer(0, 0, width, height);
    }

    pub fn commit_popup(&mut self, popup: usize) {
        self.state.popups[popup].surface.commit();
    }

    /// Unmaps the popup: a commit without a buffer.
    pub fn unmap_popup(&mut self, popup: usize) {
        let surface = &self.state.popups[popup].surface;
        surface.attach(None, 0, 0);
        surface.commit();
    }

    /// Creates the popup's `wp_fractional_scale_v1`; its preferred scales are recorded.
    pub fn get_popup_fractional_scale(&mut self, popup: usize) {
        let qh = self.queue.handle();
        let manager = self
            .state
            .fractional_scale_manager
            .as_ref()
            .expect("wp_fractional_scale_manager_v1");
        let popup_state = &mut self.state.popups[popup];
        popup_state.fractional_scale =
            Some(manager.get_fractional_scale(&popup_state.surface, &qh, PopupIndex(popup)));
    }

    /// Asks a frame callback with the popup's next commit.
    pub fn request_popup_frame(&mut self, popup: usize) {
        let qh = self.queue.handle();
        self.state.popups[popup]
            .surface
            .frame(&qh, PopupIndex(popup));
    }

    /// Asks presentation feedback for the popup's next commit.
    pub fn request_popup_feedback(&mut self, popup: usize) {
        let qh = self.queue.handle();
        let surface = &self.state.popups[popup].surface;
        self.state
            .presentation
            .as_ref()
            .expect("wp_presentation")
            .feedback(surface, &qh, PopupIndex(popup));
    }

    /// Asks an explicit grab of the keyboard with `serial`, for the popup's next commit.
    pub fn grab_popup(&mut self, popup: usize, serial: u32) {
        let seat = self.state.seat.as_ref().expect("wl_seat");
        self.state.popups[popup].popup.grab(seat, serial);
    }

    /// Asks an explicit grab with `serial` on `seat`, any `wl_seat` of the connection, for the
    /// popup's next commit.
    pub fn grab_popup_on(&mut self, popup: usize, seat: &wl_seat::WlSeat, serial: u32) {
        self.state.popups[popup].popup.grab(seat, serial);
    }

    /// Asks that the popup be placed again, as `placement` says.
    pub fn reposition_popup(&mut self, popup: usize, placement: Placement, token: u32) {
        let positioner = self.positioner(placement);
        self.state.popups[popup]
            .popup
            .reposition(&positioner, token);
        positioner.destroy();
    }

    /// An `xdg_positioner` that says `placement`.
    fn positioner(&self, placement: Placement) -> xdg_positioner::XdgPositioner {
        let qh = self.queue.handle();
        let positioner = self
            .state
            .wm_base
            .as_ref()
            .expect("xdg_wm_base")
            .create_positioner(&qh, ());
        positioner.set_size(placement.size.0, placement.size.1);
        let (x, y, width, height) = placement.anchor_rect;
        positioner.set_anchor_rect(x, y, width, height);
        positioner.set_anchor(placement.anchor);
        positioner.set_gravity(placement.gravity);
        positioner.set_constraint_adjustment(placement.adjustment);
        positioner.set_offset(placement.offset.0, placement.offset.1);
        positioner
    }

    /// The `xdg_surface` a popup on `parent` names; none for [`Parent::Unset`].
    fn parent_xdg_surface(&self, parent: Parent) -> Option<&xdg_surface::XdgSurface> {
        match parent {
            Parent::Window(window) => Some(&self.state.windows[window].xdg_surface),
            Parent::Popup(popup) => Some(&self.state.popups[popup].xdg_surface),
            Parent::Unset => None,
        }
    }

    /// Destroys the popup as a client does: its `xdg_popup`, then its `xdg_surface`, then its
    /// surface.
    pub fn destroy_popup(&mut self, popup: usize) {
        let popup = &self.state.popups[popup];
        popup.popup.destroy();
        popup.xdg_surface.destroy();
        popup.surface.destroy();
    }

    /// Destroys the popup's `xdg_popup` alone, its role object: the popup is gone, its surface
    /// and `xdg_surface` stay.
    pub fn destroy_popup_role(&mut self, popup: usize) {
        self.state.popups[popup].popup.destroy();
    }

    /// Destroys the popup's `xdg_surface` alone: before its `xdg_popup`, the client's protocol
    /// error.
    pub fn destroy_popup_xdg_surface(&mut self, popup: usize) {
        self.state.popups[popup].xdg_surface.destroy();
    }

    /// Destroys the popup's surface alone.
    pub fn destroy_popup_surface(&mut self, popup: usize) {
        self.state.popups[popup].surface.destroy();
    }

    /// Asks a popup role for the surface of `window`, a toplevel's already, on `parent`: a
    /// second role on one surface, the client's protocol error. The popup is recorded as one of
    /// its own.
    pub fn create_popup_on_window_surface(&mut self, window: usize, parent: Parent) -> usize {
        let window = &self.state.windows[window];
        let (surface, xdg_surface) = (window.surface.clone(), window.xdg_surface.clone());
        self.create_popup_role_on(surface, xdg_surface, parent)
    }

    /// Asks a second popup role for the surface of `popup`, while its first lives, on `parent`:
    /// the client's protocol error. The second is recorded as a popup of its own.
    pub fn create_popup_on_popup_surface(&mut self, popup: usize, parent: Parent) -> usize {
        let popup = &self.state.popups[popup];
        let (surface, xdg_surface) = (popup.surface.clone(), popup.xdg_surface.clone());
        self.create_popup_role_on(surface, xdg_surface, parent)
    }

    /// An `xdg_popup` of 10 × 10 on `xdg_surface`, the `xdg_surface` of `surface`, which has a
    /// role already.
    fn create_popup_role_on(
        &mut self,
        surface: wl_surface::WlSurface,
        xdg_surface: xdg_surface::XdgSurface,
        parent: Parent,
    ) -> usize {
        let qh = self.queue.handle();
        let index = self.state.popups.len();
        let positioner = self.positioner(Placement::below_right((10, 10), (0, 0, 1, 1)));
        let popup = xdg_surface.get_popup(
            self.parent_xdg_surface(parent),
            &positioner,
            &qh,
            PopupIndex(index),
        );
        positioner.destroy();
        self.state
            .popups
            .push(Popup::new(surface, xdg_surface, popup));
        index
    }
}

impl Dispatch<xdg_surface::XdgSurface, PopupIndex> for ClientState {
    fn event(
        state: &mut Self,
        _: &xdg_surface::XdgSurface,
        event: xdg_surface::Event,
        index: &PopupIndex,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_surface::Event::Configure { serial } = event {
            let popup = &mut state.popups[index.0];
            let mut configure = popup.pending;
            configure.serial = serial;
            popup.configures.push(configure);
        }
    }
}

impl Dispatch<xdg_popup::XdgPopup, PopupIndex> for ClientState {
    fn event(
        state: &mut Self,
        _: &xdg_popup::XdgPopup,
        event: xdg_popup::Event,
        index: &PopupIndex,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let popup = &mut state.popups[index.0];
        match event {
            xdg_popup::Event::Configure {
                x,
                y,
                width,
                height,
            } => {
                popup.pending = PopupConfigure {
                    serial: 0,
                    x,
                    y,
                    width,
                    height,
                };
            }
            xdg_popup::Event::PopupDone => popup.done = true,
            xdg_popup::Event::Repositioned { token } => popup.repositioned.push(token),
            _ => {}
        }
    }
}

impl Dispatch<wl_callback::WlCallback, PopupIndex> for ClientState {
    fn event(
        state: &mut Self,
        _: &wl_callback::WlCallback,
        event: wl_callback::Event,
        index: &PopupIndex,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_callback::Event::Done { callback_data } = event {
            state.popups[index.0].frames.push(callback_data);
        }
    }
}

impl Dispatch<wp_presentation_feedback::WpPresentationFeedback, PopupIndex> for ClientState {
    fn event(
        state: &mut Self,
        _: &wp_presentation_feedback::WpPresentationFeedback,
        event: wp_presentation_feedback::Event,
        index: &PopupIndex,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let Some(feedback) = Feedback::from_event(event) {
            state.popups[index.0].feedback.push(feedback);
        }
    }
}

impl Dispatch<wl_surface::WlSurface, PopupIndex> for ClientState {
    fn event(
        state: &mut Self,
        _: &wl_surface::WlSurface,
        event: wl_surface::Event,
        index: &PopupIndex,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_surface::Event::PreferredBufferScale { factor } = event {
            state.popups[index.0].preferred_buffer_scales.push(factor);
        }
    }
}

impl Dispatch<wp_fractional_scale_v1::WpFractionalScaleV1, PopupIndex> for ClientState {
    fn event(
        state: &mut Self,
        _: &wp_fractional_scale_v1::WpFractionalScaleV1,
        event: wp_fractional_scale_v1::Event,
        index: &PopupIndex,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wp_fractional_scale_v1::Event::PreferredScale { scale } = event {
            state.popups[index.0].preferred_scales.push(scale);
        }
    }
}

impl Dispatch<xdg_positioner::XdgPositioner, ()> for ClientState {
    fn event(
        _: &mut Self,
        _: &xdg_positioner::XdgPositioner,
        _: xdg_positioner::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}
