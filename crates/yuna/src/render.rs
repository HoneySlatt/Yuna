//! Drawing: the session's one renderer, Smithay's GLES on an EGL device, and what each output
//! draws ([ADR-0012](../../../docs/adr/0012-opengl-first.md),
//! [ADR-0022](../../../docs/adr/0022-the-headless-backend-in-yuna.md)).
//!
//! This is the only module that knows OpenGL; the layout and the frame loop do not.

use std::path::PathBuf;
use std::time::Instant;

use smithay::backend::allocator::Fourcc;
use smithay::backend::egl::{EGLContext, EGLDevice, EGLDisplay};
use smithay::backend::renderer::element::Kind;
use smithay::backend::renderer::element::surface::{
    WaylandSurfaceRenderElement, render_elements_from_surface_tree,
};
use smithay::backend::renderer::element::utils::CropRenderElement;
use smithay::backend::renderer::gles::{GlesError, GlesRenderbuffer, GlesRenderer, ffi};
use smithay::backend::renderer::{Bind, Color32F, ExportMem, Offscreen};
use smithay::desktop::PopupManager;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{Buffer, Logical, Physical, Point, Rectangle, Size};
use yuna_layout::geometry::Rect;

use crate::Error;
use crate::output::{VirtualOutput, to_i32};

/// The EGL device the renderer draws on.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Device {
    /// Mesa's software device, llvmpipe: the same result on every machine and in CI, and no time
    /// taken from the GPU's games.
    #[default]
    Software,
    /// The first GPU that has a render node.
    Gpu,
}

/// What an output shows where no window is.
const BACKGROUND: Color32F = Color32F::new(0.08, 0.08, 0.1, 1.0);

/// An element of a frame: part of a client's surface, cut to its window's rectangle, or to the
/// output for a popup.
pub(crate) type WindowElement = CropRenderElement<WaylandSurfaceRenderElement<GlesRenderer>>;

/// Opens the renderer on `device`, and reads the largest renderbuffer it allocates.
pub(crate) fn open_renderer(device: Device) -> Result<(GlesRenderer, u32), Error> {
    let egl_device = find_device(device)?;
    // SAFETY: Smithay keeps a registry of the EGL displays it opens, so that two `EGLDisplay`s
    // on one device share one handle and the last one dropped terminates it. That holds as long as
    // no code outside Smithay opens EGL displays in this process, and Yuna has no other EGL user.
    let display = unsafe { EGLDisplay::new(egl_device) }
        .map_err(|e| Error::Renderer(format!("cannot open the EGL display: {e}")))?;
    let context = EGLContext::new(&display)
        .map_err(|e| Error::Renderer(format!("cannot create an EGL context: {e}")))?;
    // SAFETY: the context was created just above on this thread and is current on no other. The
    // renderer never leaves this thread: the `Compositor` that owns it holds calloop's event loop,
    // which is not `Send`.
    let mut renderer = unsafe { GlesRenderer::new(context) }
        .map_err(|e| Error::Renderer(format!("cannot start the GLES renderer: {e}")))?;
    let max_size = max_renderbuffer_size(&mut renderer)?;
    Ok((renderer, max_size))
}

fn find_device(device: Device) -> Result<EGLDevice, Error> {
    let mut devices = EGLDevice::enumerate()
        .map_err(|e| Error::Renderer(format!("cannot list the EGL devices: {e}")))?;
    let found = match device {
        Device::Software => devices.find(EGLDevice::is_software),
        Device::Gpu => devices.find(|d| !d.is_software() && d.render_device_path().is_ok()),
    };
    found.ok_or_else(|| {
        Error::Renderer(match device {
            Device::Software => "no software EGL device: is Mesa's llvmpipe installed, and \
                                 __EGL_VENDOR_LIBRARY_FILENAMES pointing at Mesa?"
                .to_owned(),
            Device::Gpu => "no EGL device with a render node".to_owned(),
        })
    })
}

/// The render node of the device `renderer` draws on, such as `/dev/dri/renderD128`; refused for
/// the software device, which has none.
pub(crate) fn render_node(renderer: &GlesRenderer) -> Result<PathBuf, Error> {
    EGLDevice::device_for_display(renderer.egl_context().display())
        .and_then(|device| device.render_device_path())
        .map_err(|e| Error::Renderer(format!("no render node: {e}")))
}

/// `GL_MAX_RENDERBUFFER_SIZE`: the widest and tallest output the renderer can draw.
fn max_renderbuffer_size(renderer: &mut GlesRenderer) -> Result<u32, Error> {
    let size = renderer
        .with_context(|gl| {
            let mut size: ffi::types::GLint = 0;
            // SAFETY: `with_context` made the renderer's context current; `GetIntegerv` with this
            // name writes one integer to the pointer, which points at `size`, alive for the call.
            unsafe { gl.GetIntegerv(ffi::MAX_RENDERBUFFER_SIZE, &mut size) };
            size
        })
        .map_err(|e| Error::Renderer(format!("cannot query the renderer: {e}")))?;
    u32::try_from(size)
        .ok()
        .filter(|&size| size > 0)
        .ok_or_else(|| Error::Renderer(format!("the renderer reports a size limit of {size}")))
}

/// Allocates an output's buffer: `Abgr8888` at its physical size.
pub(crate) fn create_buffer(
    renderer: &mut GlesRenderer,
    width: u32,
    height: u32,
) -> Result<GlesRenderbuffer, Error> {
    let size: Size<i32, Buffer> = (to_i32(width), to_i32(height)).into();
    renderer
        .create_buffer(Fourcc::Abgr8888, size)
        .map_err(|e| Error::Renderer(format!("cannot allocate an output's buffer: {e}")))
}

/// A window's elements on an output, front to back: its surface tree with the window
/// geometry's top-left corner at `rect`'s, cut to `rect`.
///
/// The surface is placed on whole pixels. Where the geometry's offset in its surface is not a
/// whole number of pixels (13 logical units at scale 1.5 are 19.5 pixels), the offset is rounded
/// up: the geometry's corner lands on the rectangle's corner or up to a pixel before it, so that
/// the crop takes a sliver of content rather than show the margin, a toolkit's shadow. The far
/// edges may then show up to a pixel of margin, cut by the crop at the rectangle.
///
/// A surface whose buffer, through its viewport, maps one buffer pixel to one physical pixel, as
/// a client following its fractional scale draws it (`scale.rs`), is then copied pixel for pixel:
/// no edge blends.
pub(crate) fn window_elements(
    renderer: &mut GlesRenderer,
    surface: &WlSurface,
    geometry: Rectangle<i32, Logical>,
    rect: Rect,
    output: &VirtualOutput,
) -> Vec<WindowElement> {
    let scale = output.mode.scale();
    // `rect` lies on the output's pixel grid: its edges are whole pixels.
    let to_physical = |x: f64, y: f64| -> Point<i32, Physical> {
        (
            ((x - output.rect.x) * scale).round() as i32,
            ((y - output.rect.y) * scale).round() as i32,
        )
            .into()
    };
    let top_left = to_physical(rect.x, rect.y);
    let bottom_right = to_physical(rect.right(), rect.bottom());
    let crop = Rectangle::from_extremities(top_left, bottom_right);
    let offset: Point<i32, Physical> = (
        (f64::from(geometry.loc.x) * scale).ceil() as i32,
        (f64::from(geometry.loc.y) * scale).ceil() as i32,
    )
        .into();
    render_elements_from_surface_tree::<_, WaylandSurfaceRenderElement<GlesRenderer>>(
        renderer,
        surface,
        top_left - offset,
        scale,
        1.0,
        Kind::Unspecified,
    )
    .into_iter()
    .filter_map(|element| CropRenderElement::from_element(element, scale, crop))
    .collect()
}

/// The elements of the popups of the window whose toplevel surface is `toplevel` and whose
/// rectangle is `rect`, front to back. Each popup's window geometry has its top-left corner at
/// its place relative to the window's, rounded to the nearest pixel; its offset in its surface
/// is rounded up, as a window's is. Popups are cut to the output, not to the window: a menu may
/// overflow its window.
pub(crate) fn popup_elements(
    renderer: &mut GlesRenderer,
    toplevel: &WlSurface,
    rect: Rect,
    output: &VirtualOutput,
) -> Vec<WindowElement> {
    let scale = output.mode.scale();
    let crop =
        Rectangle::from_size((to_i32(output.mode.width()), to_i32(output.mode.height())).into());
    let mut elements = Vec::new();
    for (popup, at) in PopupManager::popups_for_surface(toplevel) {
        let corner: Point<i32, Physical> = (
            ((rect.x - output.rect.x + f64::from(at.x)) * scale).round() as i32,
            ((rect.y - output.rect.y + f64::from(at.y)) * scale).round() as i32,
        )
            .into();
        let geometry = popup.geometry();
        let offset: Point<i32, Physical> = (
            (f64::from(geometry.loc.x) * scale).ceil() as i32,
            (f64::from(geometry.loc.y) * scale).ceil() as i32,
        )
            .into();
        elements.extend(
            render_elements_from_surface_tree::<_, WaylandSurfaceRenderElement<GlesRenderer>>(
                renderer,
                popup.wl_surface(),
                corner - offset,
                scale,
                1.0,
                Kind::Unspecified,
            )
            .into_iter()
            .filter_map(|element| CropRenderElement::from_element(element, scale, crop)),
        );
    }
    elements
}

/// Draws what changed of `output`'s elements, given front to back, and waits until the drawing
/// is done. Returns whether anything was drawn.
pub(crate) fn draw(
    renderer: &mut GlesRenderer,
    output: &mut VirtualOutput,
    elements: &[WindowElement],
) -> Result<bool, Error> {
    let started = Instant::now();
    let id = output.id.0;
    let mut target = renderer
        .bind(&mut output.buffer)
        .map_err(|e| Error::Renderer(format!("cannot bind output {id}'s buffer: {e}")))?;
    // One buffer, drawn again each frame: it holds the last frame, one frame old.
    let age = usize::from(output.drawn_once);
    let result = output
        .damage
        .render_output(renderer, &mut target, age, elements, BACKGROUND)
        .map_err(|e| Error::Renderer(format!("cannot draw output {id}: {e}")))?;
    let Some(damage) = result.damage else {
        return Ok(false);
    };
    let damaged = damage.len();
    // The presentation time depends on when the drawing ends, not when it was sent.
    if result.sync.wait().is_err() {
        tracing::warn!(output = id, "waiting for a frame's drawing was interrupted");
    }
    output.drawn_once = true;
    tracing::debug!(
        output = id,
        elements = elements.len(),
        damaged_rectangles = damaged,
        took_us = started.elapsed().as_micros(),
        "frame drawn"
    );
    Ok(true)
}

/// The pixels of `region` of `output`'s last frame, as bytes R, G, B, A, row by row from the top.
pub(crate) fn read_pixels(
    renderer: &mut GlesRenderer,
    output: &mut VirtualOutput,
    region: Rectangle<i32, Physical>,
) -> Result<Vec<u8>, Error> {
    let read = |e: GlesError| Error::Renderer(format!("cannot read output {}: {e}", output.id.0));
    let target = renderer.bind(&mut output.buffer).map_err(read)?;
    let region: Rectangle<i32, Buffer> = Rectangle::new(
        (region.loc.x, region.loc.y).into(),
        (region.size.w, region.size.h).into(),
    );
    let mapping = renderer
        .copy_framebuffer(&target, region, Fourcc::Abgr8888)
        .map_err(read)?;
    let pixels = renderer.map_texture(&mapping).map_err(read)?.to_vec();
    Ok(pixels)
}
