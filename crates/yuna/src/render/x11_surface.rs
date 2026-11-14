//! X11 windows' surfaces, drawn by an element of Yuna's own
//! ([ADR-0068](../../../../docs/adr/0068-x11-windows-drawn-by-yuna-through-an-area-filter.md)).
//! M1.8's X11 window manager is to draw its windows with it; until it does, only the tests below
//! call it.
//!
//! Rootless Xwayland draws each window into a buffer of scale 1, with no viewport, at XWayland's
//! scale S: the largest scale among its seat's outputs when it started. A buffer pixel is an X11
//! pixel. On an output of scale s, an X11 pixel covers s / S physical pixels:
//!
//! - at s = S, the buffer is copied texel for texel;
//! - below S, each pixel is the average of the X11 pixels it covers, weighted by how much of
//!   each: the area filter (`area_filter.frag`), the average the captures' encoder takes
//!   (`capture.rs`);
//! - above S, on an output added since Xwayland started, the renderer's bilinear filter scales
//!   the buffer up: the right size, softer.
//!
//! Smithay's surface element is not used: it rounds a client-scaled surface's size to whole
//! logical units before scaling it, so that at S = 1.5 one width in three is drawn a pixel off,
//! and resampled even at s = S.

use std::sync::PoisonError;

use smithay::backend::renderer::element::{Element, Id, RenderElement};
use smithay::backend::renderer::gles::{
    GlesError, GlesFrame, GlesRenderer, GlesTexProgram, GlesTexture, Uniform, UniformName,
    UniformType,
};
use smithay::backend::renderer::utils::{
    CommitCounter, DamageSet, DamageSnapshot, RendererSurfaceStateUserData, import_surface,
};
use smithay::backend::renderer::{Renderer, Texture};
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::user_data::UserDataMap;
use smithay::utils::{Buffer, Physical, Point, Rectangle, Scale, Size, Transform};
use smithay::wayland::compositor::with_states;
use yuna_layout::geometry::Rect;

use super::{WindowElement, window_pixels};
use crate::output::VirtualOutput;

/// The area filter: a texture shader under Smithay's contract for custom texture shaders.
const AREA_FILTER: &str = include_str!("area_filter.frag");

/// Two scales closer than this are the same: s = S, and the buffer is copied.
const SAME_SCALE: f64 = 1e-9;

/// An X11 window's element on `output`, if any of it shows there: its surface's buffer, in X11
/// pixels, with the window geometry's corner, `geometry_offset` X11 pixels into the buffer, at
/// `rect`'s corner, cut to `rect`, at opacity `alpha`, from 0 to 1. `x11_scale` is XWayland's
/// scale, S.
///
/// The buffer takes s / S physical pixels per X11 pixel on an output of scale s, its geometry kept
/// in X11 pixels: its physical size is its size × s / S, each side rounded once, at its far edge.
/// Its geometry's corner is `rect`'s, rounded to the output's pixels; where its offset in the
/// buffer is not a whole number of pixels, the offset is rounded up, as a Wayland window's is
/// (`window_elements`). At s = S it is copied texel for texel, with no blend at its edges; below
/// S it is drawn through the area filter, exact down to s / S = 1/4, below which each pixel
/// averages the middle 4 × 4 texels of its footprint; above S, with the renderer's bilinear
/// filter.
///
/// None when the surface has no buffer, its buffer could not be imported (Smithay logs why), or
/// nothing of it shows. Only the surface itself is drawn, with its buffer scale, transform and
/// viewport ignored: Xwayland's rootless surfaces have no subsurface, and none of these but a
/// viewport, set while a fullscreen client emulates a mode
/// ([ADR-0069](../../../../docs/adr/0069-fullscreen-x11-games-gamescope-and-xwaylands-emulated-modes-kept.md)),
/// which this element does not follow yet.
pub(crate) fn x11_window_element(
    renderer: &mut GlesRenderer,
    surface: &WlSurface,
    geometry_offset: Point<i32, Buffer>,
    rect: Rect,
    x11_scale: f64,
    alpha: f32,
    output: &VirtualOutput,
) -> Option<WindowElement> {
    let texture = surface_texture(renderer, surface)?;
    X11SurfaceElement::new(
        renderer,
        texture,
        geometry_offset,
        rect,
        x11_scale,
        alpha,
        output,
    )
    .map(WindowElement::X11)
}

/// What the renderer holds of a surface's buffer.
struct SurfaceTexture {
    /// The id of the surface's elements, so that the damage tracker follows it from frame to
    /// frame.
    id: Id,
    /// Its buffer in the renderer, one texel per X11 pixel.
    texture: GlesTexture,
    /// The buffer's damage, commit by commit.
    damage: DamageSnapshot<i32, Buffer>,
}

/// `surface`'s buffer, imported into `renderer` if it was not yet. None when it has no buffer or
/// its import failed, which Smithay logs.
fn surface_texture(renderer: &mut GlesRenderer, surface: &WlSurface) -> Option<SurfaceTexture> {
    with_states(surface, |states| {
        import_surface(renderer, states).ok()?;
        let data = states
            .data_map
            .get::<RendererSurfaceStateUserData>()?
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let texture = data.texture::<GlesTexture>(renderer.context_id())?.clone();
        Some(SurfaceTexture {
            id: Id::from_wayland_resource(surface),
            texture,
            damage: data.damage(),
        })
    })
}

/// An X11 window's surface placed on one output: see [`x11_window_element`].
#[derive(Debug)]
pub(crate) struct X11SurfaceElement {
    id: Id,
    texture: GlesTexture,
    damage: DamageSnapshot<i32, Buffer>,
    placement: Placement,
    alpha: f32,
    filter: Filter,
}

/// Where an X11 buffer is drawn on an output, in its physical pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Placement {
    /// The whole buffer, at s / S physical pixels per X11 pixel.
    buffer: Rectangle<i32, Physical>,
    /// What is drawn of it: its part inside its window's rectangle. Never empty.
    shown: Rectangle<i32, Physical>,
}

/// How an X11 buffer's texels become an output's pixels.
#[derive(Debug, Clone)]
enum Filter {
    /// At XWayland's scale: each pixel's centre falls on its texel's, through the renderer's own
    /// shader.
    Copy,
    /// Below it: the area filter's program.
    Area(GlesTexProgram),
    /// Above it, or below it when the area filter did not compile: the renderer's own shader and
    /// its bilinear filter.
    Bilinear,
}

impl X11SurfaceElement {
    /// The element of `surface` placed on `output`, as [`x11_window_element`] says; none when
    /// nothing of it shows.
    fn new(
        renderer: &mut GlesRenderer,
        surface: SurfaceTexture,
        geometry_offset: Point<i32, Buffer>,
        rect: Rect,
        x11_scale: f64,
        alpha: f32,
        output: &VirtualOutput,
    ) -> Option<X11SurfaceElement> {
        let placement = place(
            surface.texture.size(),
            geometry_offset,
            rect,
            x11_scale,
            output,
        )?;
        Some(X11SurfaceElement {
            id: surface.id,
            texture: surface.texture,
            damage: surface.damage,
            placement,
            alpha,
            filter: filter(renderer, output.mode.scale(), x11_scale),
        })
    }
}

/// Where a buffer of `buffer_size` X11 pixels is drawn on `output`, as [`x11_window_element`]
/// says. None when nothing of it shows: an empty buffer, an empty rectangle, a buffer whose size
/// rounds to no pixel, or one wholly outside its rectangle.
fn place(
    buffer_size: Size<i32, Buffer>,
    geometry_offset: Point<i32, Buffer>,
    rect: Rect,
    x11_scale: f64,
    output: &VirtualOutput,
) -> Option<Placement> {
    let scale = output.mode.scale();
    let same_scale = same_scale(scale, x11_scale);
    // At s = S an X11 pixel is a physical pixel, exactly: no rounding may move an edge.
    let in_pixels = |x11: i32| -> f64 {
        if same_scale {
            f64::from(x11)
        } else {
            f64::from(x11) * scale / x11_scale
        }
    };
    let window = window_pixels(rect, output);
    let offset: Point<i32, Physical> = (
        in_pixels(geometry_offset.x).ceil() as i32,
        in_pixels(geometry_offset.y).ceil() as i32,
    )
        .into();
    let size: Size<i32, Physical> = (
        in_pixels(buffer_size.w).round() as i32,
        in_pixels(buffer_size.h).round() as i32,
    )
        .into();
    let buffer = Rectangle::new(window.loc - offset, size);
    let shown = buffer.intersection(window)?;
    Some(Placement { buffer, shown })
}

/// Whether an output of scale `scale` is at XWayland's scale `x11_scale`, up to rounding.
fn same_scale(scale: f64, x11_scale: f64) -> bool {
    (scale - x11_scale).abs() < SAME_SCALE
}

/// The filter for a buffer drawn at XWayland's scale `x11_scale` on an output of scale `scale`.
fn filter(renderer: &mut GlesRenderer, scale: f64, x11_scale: f64) -> Filter {
    if same_scale(scale, x11_scale) {
        Filter::Copy
    } else if scale < x11_scale {
        area_filter(renderer).map_or(Filter::Bilinear, Filter::Area)
    } else {
        Filter::Bilinear
    }
}

/// The area filter's program for one renderer, kept in its EGL context's user data: compiled at
/// the first X11 surface drawn below XWayland's scale. None when it did not compile, so that it is
/// tried and logged once.
struct AreaFilterProgram(Option<GlesTexProgram>);

/// `renderer`'s area filter, compiled once; none if it does not compile.
fn area_filter(renderer: &mut GlesRenderer) -> Option<GlesTexProgram> {
    if let Some(AreaFilterProgram(kept)) = renderer.egl_context().user_data().get() {
        return kept.clone();
    }
    let program = compile_area_filter(renderer, AREA_FILTER);
    // Kept for this thread alone, which is enough: the renderer never leaves its thread.
    renderer
        .egl_context()
        .user_data()
        .insert_if_missing(|| AreaFilterProgram(program.clone()));
    program
}

/// `source` compiled as the area filter's program, with its uniforms: none, logged, if it does
/// not compile. X11 windows below XWayland's scale are then drawn with the bilinear filter.
fn compile_area_filter(renderer: &mut GlesRenderer, source: &str) -> Option<GlesTexProgram> {
    let uniforms = [
        UniformName::new("tex_size", UniformType::_2f),
        UniformName::new("ratio", UniformType::_2f),
    ];
    renderer
        .compile_custom_texture_shader(source, &uniforms)
        .inspect_err(|error| {
            tracing::warn!(
                %error,
                "the area filter did not compile: X11 windows below XWayland's scale are drawn \
                 with the bilinear filter"
            );
        })
        .ok()
}

impl Element for X11SurfaceElement {
    fn id(&self) -> &Id {
        &self.id
    }

    fn current_commit(&self) -> CommitCounter {
        self.damage.current_commit()
    }

    /// The part of the buffer shown, in X11 pixels: `shown` mapped back into the buffer.
    fn src(&self) -> Rectangle<f64, Buffer> {
        let Placement { buffer, shown } = self.placement;
        let size = self.texture.size();
        let to_texels_x =
            |pixels: i32| f64::from(pixels) * f64::from(size.w) / f64::from(buffer.size.w);
        let to_texels_y =
            |pixels: i32| f64::from(pixels) * f64::from(size.h) / f64::from(buffer.size.h);
        Rectangle::new(
            (
                to_texels_x(shown.loc.x - buffer.loc.x),
                to_texels_y(shown.loc.y - buffer.loc.y),
            )
                .into(),
            (to_texels_x(shown.size.w), to_texels_y(shown.size.h)).into(),
        )
    }

    /// Where it is drawn. It was placed for one output, at that output's scale, which the
    /// damage tracker passes again as `scale`.
    fn geometry(&self, _scale: Scale<f64>) -> Rectangle<i32, Physical> {
        self.placement.shown
    }

    /// The buffer's damage since `commit`, mapped to the output's pixels, rounded outward and
    /// cut to what is shown, relative to it; all of it when `commit` is too old or none.
    fn damage_since(
        &self,
        _scale: Scale<f64>,
        commit: Option<CommitCounter>,
    ) -> DamageSet<i32, Physical> {
        let size = self.texture.size();
        let Placement { buffer, shown } = self.placement;
        // How far past a pixel's footprint its drawing reads, in texels. The bilinear filter
        // reads up to a texel past it. The area filter reads each texel just past its centre,
        // where a sampler finer than 1/256 of a texel takes 1/1024 of the next one: enough to
        // change a rounding. The copy reads each texel at its centre, its neighbours' weight too
        // small to change one.
        let reach = match self.filter {
            Filter::Area(_) | Filter::Bilinear => 1.0,
            Filter::Copy => 0.0,
        };
        let to_pixels_x = |texels: f64| texels * f64::from(buffer.size.w) / f64::from(size.w);
        let to_pixels_y = |texels: f64| texels * f64::from(buffer.size.h) / f64::from(size.h);
        let shown_in_buffer = Rectangle::new(shown.loc - buffer.loc, shown.size);
        self.damage
            .damage_since(commit)
            .unwrap_or_else(|| DamageSet::from_slice(&[Rectangle::from_size(size)]))
            .iter()
            .filter_map(|damaged| {
                let (x, y) = (f64::from(damaged.loc.x), f64::from(damaged.loc.y));
                let (w, h) = (f64::from(damaged.size.w), f64::from(damaged.size.h));
                let pixels = Rectangle::<i32, Physical>::from_extremities(
                    (
                        to_pixels_x(x - reach).floor() as i32,
                        to_pixels_y(y - reach).floor() as i32,
                    ),
                    (
                        to_pixels_x(x + w + reach).ceil() as i32,
                        to_pixels_y(y + h + reach).ceil() as i32,
                    ),
                );
                let cut = pixels.intersection(shown_in_buffer)?;
                Some(Rectangle::new(cut.loc - shown_in_buffer.loc, cut.size))
            })
            .collect()
    }

    fn alpha(&self) -> f32 {
        self.alpha
    }
}

impl RenderElement<GlesRenderer> for X11SurfaceElement {
    fn draw(
        &self,
        frame: &mut GlesFrame<'_, '_>,
        src: Rectangle<f64, Buffer>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        opaque_regions: &[Rectangle<i32, Physical>],
        _cache: Option<&UserDataMap>,
    ) -> Result<(), GlesError> {
        let Filter::Area(program) = &self.filter else {
            return frame.render_texture_from_to(
                &self.texture,
                src,
                dst,
                damage,
                opaque_regions,
                Transform::Normal,
                self.alpha,
                None,
                &[],
            );
        };
        let size = self.texture.size();
        let uniforms = [
            Uniform::new("tex_size", (size.w as f32, size.h as f32)),
            Uniform::new(
                "ratio",
                (
                    (src.size.w / f64::from(dst.size.w)) as f32,
                    (src.size.h / f64::from(dst.size.h)) as f32,
                ),
            ),
        ];
        frame.render_texture_from_to(
            &self.texture,
            src,
            dst,
            damage,
            opaque_regions,
            Transform::Normal,
            self.alpha,
            Some(program),
            &uniforms,
        )
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use smithay::backend::allocator::Fourcc;
    use smithay::backend::renderer::ImportMem;
    use smithay::backend::renderer::utils::DamageBag;
    use smithay::reexports::wayland_server::Display;
    use yuna_motion::Settings;

    use super::*;
    use crate::capture::axis_taps;
    use crate::clock::ClockMode;
    use crate::output::OutputMode;
    use crate::render::{Device, draw, read_pixels};
    use crate::state::{OWNER_SEAT, State};

    const WHITE: [u8; 4] = [255, 255, 255, 255];
    const BLACK: [u8; 4] = [0, 0, 0, 255];
    const RED: [u8; 4] = [255, 0, 0, 255];
    const BLUE: [u8; 4] = [0, 0, 255, 255];

    /// How many 8-bit levels a pixel the area filter draws may differ from the reference, the
    /// exact average rounded. The filter sums its weights in 32-bit floats and reads each texel
    /// alone, or with 1/1024 of the next on a sampler finer than 1/256 of a texel
    /// (`area_filter.frag`): before the renderer rounds it, its value is within a quarter of a
    /// level of the exact average, so the two rounded values differ by one level at most. On
    /// llvmpipe they differ where the average's fraction lies from 0.5 to 0.6: it stores those a
    /// level low.
    const TOLERANCE: u8 = 1;

    /// An image of R, G, B, A pixels, row by row, as a test gives an X11 window's buffer.
    struct Image {
        width: i32,
        height: i32,
        pixels: Vec<[u8; 4]>,
    }

    impl Image {
        fn new(width: i32, height: i32, pixel: impl Fn(i32, i32) -> [u8; 4]) -> Image {
            let pixels = (0..height)
                .flat_map(|y| (0..width).map(move |x| (x, y)))
                .map(|(x, y)| pixel(x, y))
                .collect();
            Image {
                width,
                height,
                pixels,
            }
        }

        fn at(&self, x: i32, y: i32) -> [u8; 4] {
            self.pixels[(y * self.width + x) as usize]
        }
    }

    /// The same pixel at (x, y) at every call, and unlike its neighbours'.
    fn noise(x: i32, y: i32) -> [u8; 4] {
        let mut hash = (x as u32).wrapping_mul(0x9E37_79B9) ^ (y as u32).wrapping_mul(0x85EB_CA6B);
        hash ^= hash >> 15;
        hash = hash.wrapping_mul(0x2C1B_3C6D);
        hash ^= hash >> 12;
        let [r, g, b, _] = hash.to_le_bytes();
        [r, g, b, 255]
    }

    /// 1000 × 1000 X11 pixels of a one-pixel checkerboard, crossed every 50 pixels by
    /// one-pixel lines of colour, around a square of noise: a window's text and rules at their
    /// sharpest.
    fn sharp_image() -> Image {
        Image::new(1000, 1000, |x, y| {
            if x % 50 == 7 {
                [255, 64, 0, 255]
            } else if y % 50 == 13 {
                [0, 160, 255, 255]
            } else if (400..600).contains(&x) && (400..600).contains(&y) {
                noise(x, y)
            } else if (x + y) % 2 == 0 {
                WHITE
            } else {
                BLACK
            }
        })
    }

    /// The renderer and outputs of a compositor on llvmpipe, and the display their globals live
    /// on, dropped after them.
    struct Scene {
        state: State,
        _display: Display<State>,
    }

    impl Scene {
        /// Outputs of `width` × `height` pixels at `scale`, laid out left to right, each drawn
        /// once with no window: its background.
        fn new(outputs: &[(u32, u32, f64)]) -> Scene {
            let display = Display::<State>::new().expect("a Wayland display");
            let state = State::new(
                display.handle(),
                ClockMode::Manual,
                Device::Software,
                false,
                Settings::default(),
            )
            .expect("llvmpipe's renderer");
            let mut scene = Scene {
                state,
                _display: display,
            };
            for (index, &(width, height, scale)) in outputs.iter().enumerate() {
                let mode = OutputMode::new(width, height, 60.0, scale).expect("a valid mode");
                scene.state.add_output(OWNER_SEAT, mode).expect("an output");
                scene.draw(index, &[]);
            }
            scene
        }

        /// `image` as a buffer of `format`: `Xrgb8888`, as Xwayland gives an opaque window, or
        /// `Argb8888`, premultiplied.
        fn texture(&mut self, image: &Image, format: Fourcc) -> GlesTexture {
            let bytes: Vec<u8> = image
                .pixels
                .iter()
                .flat_map(|&[r, g, b, a]| [b, g, r, a])
                .collect();
            self.state
                .renderer
                .import_memory(&bytes, format, (image.width, image.height).into(), false)
                .expect("a texture")
        }

        /// The element of a surface whose buffer is `texture`, its window at `rect` with no
        /// geometry offset, on output `output`.
        fn element(
            &mut self,
            texture: GlesTexture,
            rect: Rect,
            x11_scale: f64,
            alpha: f32,
            output: usize,
        ) -> Option<X11SurfaceElement> {
            let surface = SurfaceTexture {
                id: Id::new(),
                texture,
                damage: DamageSnapshot::empty(),
            };
            X11SurfaceElement::new(
                &mut self.state.renderer,
                surface,
                Point::default(),
                rect,
                x11_scale,
                alpha,
                &self.state.outputs[output],
            )
        }

        /// Draws what changed of output `output` with `elements`, front to back.
        fn draw(&mut self, output: usize, elements: &[WindowElement]) {
            draw(
                &mut self.state.renderer,
                &mut self.state.outputs[output],
                elements,
            )
            .expect("a frame drawn");
        }

        /// Draws output `output` with `element` alone.
        fn draw_one(&mut self, output: usize, element: X11SurfaceElement) {
            self.draw(output, &[WindowElement::X11(element)]);
        }

        /// The pixels of `region` of output `output`'s last frame, R, G, B, A, row by row.
        fn read(&mut self, output: usize, region: Rectangle<i32, Physical>) -> Vec<[u8; 4]> {
            read_pixels(
                &mut self.state.renderer,
                &mut self.state.outputs[output],
                region,
            )
            .expect("the output's pixels")
            .as_chunks::<4>()
            .0
            .to_vec()
        }

        /// The colour of output `output` at (x, y), where no window is.
        fn background(&mut self, output: usize, x: i32, y: i32) -> [u8; 4] {
            self.read(output, Rectangle::new((x, y).into(), (1, 1).into()))[0]
        }
    }

    /// What the area filter gives for `image` drawn at `drawn` pixels, on `part` of them: each
    /// pixel the average of the texels under it with the weights of the captures' encoder
    /// (`axis_taps`), rounded to 8 bits. R, G, B, row by row.
    fn area_average(
        image: &Image,
        drawn: Size<i32, Physical>,
        part: Rectangle<i32, Physical>,
    ) -> Vec<[u8; 3]> {
        let taps = |origin: i32, count: i32, texels: i32, pixels: i32| {
            axis_taps(
                f64::from(origin),
                count as u32,
                f64::from(texels) / f64::from(pixels),
                0,
                texels as usize,
            )
        };
        let columns = taps(part.loc.x, part.size.w, image.width, drawn.w);
        let rows = taps(part.loc.y, part.size.h, image.height, drawn.h);
        let mut averages = Vec::with_capacity(columns.len() * rows.len());
        for (first_row, row_weights) in &rows {
            for (first_column, column_weights) in &columns {
                let mut sum = [0.0f64; 3];
                for (dy, row_weight) in row_weights.iter().enumerate() {
                    for (dx, column_weight) in column_weights.iter().enumerate() {
                        let texel = image.at((first_column + dx) as i32, (first_row + dy) as i32);
                        let weight = f64::from(*row_weight) * f64::from(*column_weight);
                        for (total, value) in sum.iter_mut().zip(texel) {
                            *total += weight * f64::from(value);
                        }
                    }
                }
                averages.push(sum.map(|total| total.round().clamp(0.0, 255.0) as u8));
            }
        }
        averages
    }

    /// The largest difference of `drawn` from `expected` on any channel, after checking each
    /// pixel is opaque; prints it, with how many pixels differ, under `what`.
    fn largest_difference(what: &str, drawn: &[[u8; 4]], expected: &[[u8; 3]]) -> u8 {
        assert_eq!(drawn.len(), expected.len(), "{what}");
        let mut largest = 0;
        let mut differing = 0;
        for (pixel, wanted) in drawn.iter().zip(expected) {
            assert_eq!(pixel[3], 255, "{what}: an opaque window stays opaque");
            let difference = (0..3)
                .map(|c| pixel[c].abs_diff(wanted[c]))
                .max()
                .unwrap_or(0);
            largest = largest.max(difference);
            differing += usize::from(difference > 0);
        }
        println!(
            "{what}: largest difference {largest}, {differing} of {} pixels differ",
            drawn.len()
        );
        largest
    }

    /// Draws `image` at `x11_scale` on output 0 of `scene`, whose scale is lower, its window over
    /// the whole output, and checks it is drawn at `size` pixels, each the area average.
    fn assert_area_filtered(
        scene: &mut Scene,
        image: &Image,
        x11_scale: f64,
        size: (i32, i32),
        what: &str,
    ) {
        let rect = scene.state.outputs[0].rect;
        let texture = scene.texture(image, Fourcc::Xrgb8888);
        let element = scene
            .element(texture, rect, x11_scale, 1.0, 0)
            .expect("an element");
        assert!(matches!(element.filter, Filter::Area(_)), "{what}");
        let size = Size::from(size);
        assert_eq!(element.placement.buffer.size, size, "{what}");
        scene.draw_one(0, element);
        let whole = Rectangle::from_size(size);
        let drawn = scene.read(0, whole);
        let expected = area_average(image, size, whole);
        assert!(largest_difference(what, &drawn, &expected) <= TOLERANCE);
    }

    #[test]
    fn x11_element_is_texel_for_texel_at_the_x11_scale() {
        let image = sharp_image();
        let mut scene = Scene::new(&[(1024, 1024, 1.5)]);
        let background = scene.background(0, 1023, 1023);
        // A corner on whole pixels, and one at (10.5, 7.5) that rounds to (11, 8). Each window is
        // 10 pixels wider and taller than its buffer, so that the cut does not hide an edge.
        for ((x, y), corner) in [((2.0, 4.0), (3, 6)), ((7.0, 5.0), (11, 8))] {
            let texture = scene.texture(&image, Fourcc::Xrgb8888);
            let rect = Rect::new(x, y, 1010.0 / 1.5, 1010.0 / 1.5);
            let element = scene
                .element(texture, rect, 1.5, 1.0, 0)
                .expect("an element");
            assert!(matches!(element.filter, Filter::Copy));
            scene.draw_one(0, element);
            // The buffer and a ring of a pixel around it.
            let ring = Rectangle::new((corner.0 - 1, corner.1 - 1).into(), (1002, 1002).into());
            let drawn = scene.read(0, ring);
            for (i, pixel) in drawn.iter().enumerate() {
                let (x, y) = ((i % 1002) as i32 - 1, (i / 1002) as i32 - 1);
                let expected = if (0..1000).contains(&x) && (0..1000).contains(&y) {
                    image.at(x, y)
                } else {
                    background
                };
                assert_eq!(*pixel, expected, "at {x}, {y} of the buffer at {corner:?}");
            }
        }
    }

    #[test]
    fn x11_element_area_filters_at_two_thirds() {
        let image = sharp_image();
        let mut scene = Scene::new(&[(700, 700, 1.0)]);
        let texture = scene.texture(&image, Fourcc::Xrgb8888);
        let element = scene
            .element(texture, Rect::new(3.0, 5.0, 680.0, 680.0), 1.5, 1.0, 0)
            .expect("an element");
        assert!(matches!(element.filter, Filter::Area(_)));
        // 1000 × 2/3 = 666.67 pixels, rounded once.
        let placed = Rectangle::new((3, 5).into(), (667, 667).into());
        assert_eq!(element.placement.buffer, placed);
        scene.draw_one(0, element);
        let drawn = scene.read(0, placed);
        let expected = area_average(&image, placed.size, Rectangle::from_size(placed.size));
        assert!(largest_difference("2/3", &drawn, &expected) <= TOLERANCE);

        // A one-pixel white line on black, at 30 places across and down: at every phase of a
        // pixel's 1.5 texels, whole in one pixel or shared by two, it keeps 2/3 of its
        // intensity, 170 of 255. The bilinear filter gives every third texel half the weight of
        // its neighbours: a line shows as one pixel at 75 % or two at 25 %, and flickers as it
        // slides (ADR-0068).
        for place in 0..30 {
            for across in [true, false] {
                let line = if across {
                    Image::new(30, 3, |x, _| if x == place { WHITE } else { BLACK })
                } else {
                    Image::new(3, 30, |_, y| if y == place { WHITE } else { BLACK })
                };
                let texture = scene.texture(&line, Fourcc::Xrgb8888);
                let element = scene
                    .element(texture, Rect::new(0.0, 0.0, 30.0, 30.0), 1.5, 1.0, 0)
                    .expect("an element");
                scene.draw_one(0, element);
                let pixels = if across {
                    scene.read(0, Rectangle::from_size((20, 1).into()))
                } else {
                    scene.read(0, Rectangle::from_size((1, 20).into()))
                };
                let total: i32 = pixels.iter().map(|pixel| i32::from(pixel[0])).sum();
                let lit = pixels.iter().filter(|pixel| pixel[0] > 0).count();
                assert!(
                    (total - 170).abs() <= 2 * i32::from(TOLERANCE) && (1..=2).contains(&lit),
                    "a line at {place}, across {across}: {total} over {lit} pixels"
                );
            }
        }
    }

    #[test]
    fn x11_element_size_is_buffer_times_s_over_s_max() {
        let mut scene = Scene::new(&[(1024, 3, 1.5), (1024, 3, 1.0)]);
        let background = [scene.background(0, 1023, 2), scene.background(1, 1023, 2)];
        for (output, scale) in [(0, 1.5), (1, 1.0)] {
            for width in 1..=1000 {
                // width × s / S: at 1.5, the width; at 1, width × 2/3 rounded, never a half.
                let expected: i32 = if output == 0 {
                    width
                } else {
                    (2 * width + 1) / 3
                };
                let image = Image::new(width, 3, |_, _| WHITE);
                let texture = scene.texture(&image, Fourcc::Xrgb8888);
                // The window's rectangle is the whole output: it cuts nothing.
                let rect = scene.state.outputs[output].rect;
                let element = scene
                    .element(texture, rect, 1.5, 1.0, output)
                    .expect("an element");
                let height = if output == 0 { 3 } else { 2 };
                assert_eq!(
                    element.placement.buffer,
                    Rectangle::from_size((expected, height).into()),
                    "{width} X11 pixels at {scale}"
                );
                scene.draw_one(output, element);
                let row = scene.read(output, Rectangle::from_size((1024, 1).into()));
                let drawn = row.iter().take_while(|pixel| **pixel == WHITE).count();
                assert_eq!(drawn, expected as usize, "{width} X11 pixels at {scale}");
                assert!(
                    row[drawn..]
                        .iter()
                        .all(|pixel| *pixel == background[output]),
                    "{width} X11 pixels at {scale}: nothing blends past the far edge"
                );
            }
        }
    }

    #[test]
    fn x11_element_of_an_empty_buffer_is_none() {
        let mut scene = Scene::new(&[(64, 64, 1.0)]);
        let rect = Rect::new(0.0, 0.0, 64.0, 64.0);
        let output = &scene.state.outputs[0];
        for size in [(0, 0), (0, 10), (10, 0)] {
            assert_eq!(
                place(size.into(), Point::default(), rect, 1.5, output),
                None
            );
        }
        // One X11 pixel at S = 4 is a quarter of a pixel at 1: it rounds to none.
        assert_eq!(
            place((1, 1).into(), Point::default(), rect, 4.0, output),
            None
        );
        // A window with no area shows nothing of its buffer.
        let flat = Rect::new(5.0, 5.0, 0.0, 10.0);
        assert_eq!(
            place((10, 10).into(), Point::default(), flat, 1.5, output),
            None
        );
        // Through the element, an empty buffer gives none, and nothing panics.
        let empty = Image::new(0, 0, |_, _| WHITE);
        let texture = scene.texture(&empty, Fourcc::Xrgb8888);
        assert!(scene.element(texture, rect, 1.5, 1.0, 0).is_none());
    }

    #[test]
    fn x11_element_larger_than_the_output_is_cut_to_it() {
        // Copied: 1000 × 800 X11 pixels at S = 1 on an output of 640 × 480 at 1.
        let image = Image::new(1000, 800, noise);
        let mut scene = Scene::new(&[(640, 480, 1.0)]);
        let texture = scene.texture(&image, Fourcc::Xrgb8888);
        let element = scene
            .element(texture, Rect::new(0.0, 0.0, 1000.0, 800.0), 1.0, 1.0, 0)
            .expect("an element");
        scene.draw_one(0, element);
        let output = Rectangle::from_size((640, 480).into());
        let drawn = scene.read(0, output);
        for (i, pixel) in drawn.iter().enumerate() {
            let (x, y) = ((i % 640) as i32, (i / 640) as i32);
            assert_eq!(*pixel, image.at(x, y), "at {x}, {y}");
        }

        // Filtered: 1600 × 1200 X11 pixels at S = 1.5, 1067 × 800 pixels, cut to the output.
        let image = Image::new(1600, 1200, noise);
        let texture = scene.texture(&image, Fourcc::Xrgb8888);
        let element = scene
            .element(texture, Rect::new(0.0, 0.0, 1067.0, 800.0), 1.5, 1.0, 0)
            .expect("an element");
        let size = element.placement.buffer.size;
        assert_eq!(size, (1067, 800).into());
        scene.draw_one(0, element);
        let drawn = scene.read(0, output);
        let expected = area_average(&image, size, output);
        assert!(largest_difference("larger than the output", &drawn, &expected) <= TOLERANCE);
    }

    #[test]
    fn x11_element_partly_off_the_output_shows_its_part_on_it() {
        let image = Image::new(300, 300, noise);
        let mut scene = Scene::new(&[(400, 300, 1.5), (400, 300, 1.0)]);
        // At s = S, its corner 20 by 10 logical units off the output's, 30 by 15 pixels.
        let output = scene.state.outputs[0].rect;
        let rect = Rect::new(output.x - 20.0, output.y - 10.0, 200.0, 200.0);
        let texture = scene.texture(&image, Fourcc::Xrgb8888);
        let element = scene
            .element(texture, rect, 1.5, 1.0, 0)
            .expect("an element");
        assert_eq!(element.placement.buffer.loc, (-30, -15).into());
        scene.draw_one(0, element);
        let drawn = scene.read(0, Rectangle::from_size((270, 285).into()));
        for (i, pixel) in drawn.iter().enumerate() {
            let (x, y) = ((i % 270) as i32, (i / 270) as i32);
            assert_eq!(*pixel, image.at(x + 30, y + 15), "at {x}, {y}");
        }

        // Below S, 20 by 10 pixels off: what shows is the area average of its part on the
        // output, its texels' phases those of the whole buffer.
        let output = scene.state.outputs[1].rect;
        let rect = Rect::new(output.x - 20.0, output.y - 10.0, 200.0, 200.0);
        let texture = scene.texture(&image, Fourcc::Xrgb8888);
        let element = scene
            .element(texture, rect, 1.5, 1.0, 1)
            .expect("an element");
        assert_eq!(
            element.placement.buffer,
            Rectangle::new((-20, -10).into(), (200, 200).into())
        );
        scene.draw_one(1, element);
        let drawn = scene.read(1, Rectangle::from_size((180, 190).into()));
        let expected = area_average(
            &image,
            (200, 200).into(),
            Rectangle::new((20, 10).into(), (180, 190).into()),
        );
        assert!(largest_difference("partly off the output", &drawn, &expected) <= TOLERANCE);
    }

    #[test]
    fn x11_element_cut_to_its_window_keeps_its_geometry_offset_in_x11_pixels() {
        // The window's geometry starts 30 by 15 X11 pixels into its buffer, a shadow's margin:
        // 20 by 10 pixels at 1 under S = 1.5. Its rectangle, 100 × 80 at (10, 10), cuts the
        // buffer's 200 × 200 pixels at (-10, 0) to the rectangle.
        let image = Image::new(300, 300, noise);
        let mut scene = Scene::new(&[(200, 200, 1.0)]);
        let background = scene.background(0, 199, 199);
        let texture = scene.texture(&image, Fourcc::Xrgb8888);
        let surface = SurfaceTexture {
            id: Id::new(),
            texture,
            damage: DamageSnapshot::empty(),
        };
        let element = X11SurfaceElement::new(
            &mut scene.state.renderer,
            surface,
            (30, 15).into(),
            Rect::new(10.0, 10.0, 100.0, 80.0),
            1.5,
            1.0,
            &scene.state.outputs[0],
        )
        .expect("an element");
        let placement = element.placement;
        assert_eq!(
            placement.buffer,
            Rectangle::new((-10, 0).into(), (200, 200).into())
        );
        assert_eq!(
            placement.shown,
            Rectangle::new((10, 10).into(), (100, 80).into())
        );
        scene.draw_one(0, element);
        let drawn = scene.read(0, placement.shown);
        let expected = area_average(
            &image,
            (200, 200).into(),
            Rectangle::new((20, 10).into(), (100, 80).into()),
        );
        assert!(largest_difference("cut to its window", &drawn, &expected) <= TOLERANCE);
        // Nothing past the rectangle's edges.
        for (x, y) in [(9, 50), (110, 50), (50, 9), (50, 90)] {
            assert_eq!(scene.background(0, x, y), background, "at {x}, {y}");
        }
    }

    #[test]
    fn x11_element_at_half_opacity_blends_with_what_is_below() {
        let mut scene = Scene::new(&[(64, 64, 1.0)]);
        let below = scene.background(0, 63, 63);
        let rect = Rect::new(8.0, 8.0, 32.0, 32.0);
        let blend = |colour: u8, alpha: f64, below: u8, opacity: f64| -> u8 {
            (f64::from(colour) * opacity + f64::from(below) * (1.0 - alpha * opacity)).round() as u8
        };
        // Opaque X11 windows, and a premultiplied one at alpha 128, at S = 1 (copied), 1.5
        // (the area filter) and 0.5 (the bilinear filter).
        let opaque = [200, 100, 50, 255];
        let translucent = [100, 50, 25, 128];
        for (colour, format) in [(opaque, Fourcc::Xrgb8888), (translucent, Fourcc::Argb8888)] {
            for x11_scale in [1.0, 1.5, 0.5] {
                let image = Image::new(48, 48, |_, _| colour);
                let texture = scene.texture(&image, format);
                let element = scene
                    .element(texture, rect, x11_scale, 0.5, 0)
                    .expect("an element");
                scene.draw_one(0, element);
                let pixel = scene.background(0, 12, 12);
                let alpha = f64::from(colour[3]) / 255.0;
                for c in 0..3 {
                    let expected = blend(colour[c], alpha, below[c], 0.5);
                    assert!(
                        pixel[c].abs_diff(expected) <= TOLERANCE,
                        "{format:?} at S = {x11_scale}: {pixel:?}, channel {c} not {expected}"
                    );
                }
            }
        }
    }

    #[test]
    fn x11_element_at_scale_one_under_one_is_a_plain_copy() {
        let image = Image::new(256, 256, noise);
        let mut scene = Scene::new(&[(300, 300, 1.0)]);
        let texture = scene.texture(&image, Fourcc::Xrgb8888);
        let element = scene
            .element(texture, Rect::new(0.0, 0.0, 300.0, 300.0), 1.0, 1.0, 0)
            .expect("an element");
        assert!(matches!(element.filter, Filter::Copy));
        scene.draw_one(0, element);
        let drawn = scene.read(0, Rectangle::from_size((256, 256).into()));
        for (i, pixel) in drawn.iter().enumerate() {
            let (x, y) = ((i % 256) as i32, (i / 256) as i32);
            assert_eq!(*pixel, image.at(x, y), "at {x}, {y}");
        }
    }

    #[test]
    fn x11_element_area_filters_at_1_25_under_1_5() {
        // 301 × 300 X11 pixels: 250.83 × 250 pixels, 251 × 250 once rounded.
        let image = Image::new(301, 300, noise);
        let mut scene = Scene::new(&[(400, 400, 1.25)]);
        assert_area_filtered(&mut scene, &image, 1.5, (251, 250), "1.25 under 1.5");
    }

    #[test]
    fn x11_element_area_filters_at_one_under_two() {
        // 300 × 301 X11 pixels: 150 × 150.5 pixels, 150 × 151 once rounded.
        let image = Image::new(300, 301, noise);
        let mut scene = Scene::new(&[(400, 400, 1.0)]);
        assert_area_filtered(&mut scene, &image, 2.0, (150, 151), "1 under 2");
    }

    #[test]
    fn x11_element_area_filters_down_to_a_quarter_and_narrows_below() {
        // At 1/4, a footprint of 4 texels, 5 at most phases: averaged whole. 203 × 200 X11
        // pixels: 50.75 × 50 pixels, 51 × 50 once rounded.
        let image = Image::new(203, 200, noise);
        let mut scene = Scene::new(&[(400, 400, 1.0)]);
        assert_area_filtered(&mut scene, &image, 4.0, (51, 50), "1 under 4");

        // At 1/8 a footprint is narrowed to its middle 4 texels: a plain colour stays itself.
        let image = Image::new(160, 160, |_, _| [90, 180, 30, 255]);
        let texture = scene.texture(&image, Fourcc::Xrgb8888);
        let element = scene
            .element(texture, Rect::new(0.0, 0.0, 400.0, 400.0), 8.0, 1.0, 0)
            .expect("an element");
        scene.draw_one(0, element);
        let drawn = scene.read(0, Rectangle::from_size((20, 20).into()));
        assert!(drawn.iter().all(|pixel| *pixel == [90, 180, 30, 255]));
    }

    #[test]
    fn x11_element_scales_up_bilinearly_above_the_x11_scale() {
        // 300 X11 pixels at S = 1.5 on an output at 2: 400 pixels. Red on the left half, blue
        // on the right.
        let image = Image::new(300, 300, |x, _| if x < 150 { RED } else { BLUE });
        let mut scene = Scene::new(&[(500, 500, 2.0)]);
        let background = scene.background(0, 499, 499);
        let texture = scene.texture(&image, Fourcc::Xrgb8888);
        let element = scene
            .element(texture, Rect::new(0.0, 0.0, 250.0, 250.0), 1.5, 1.0, 0)
            .expect("an element");
        assert!(matches!(element.filter, Filter::Bilinear));
        assert_eq!(
            element.placement.buffer,
            Rectangle::from_size((400, 400).into())
        );
        scene.draw_one(0, element);
        let row = scene.read(0, Rectangle::new((0, 200).into(), (401, 1).into()));
        // Its edges take nothing from outside it; the colours meet in a blend of a few pixels.
        assert!(row[..198].iter().all(|pixel| *pixel == RED));
        assert!(row[202..400].iter().all(|pixel| *pixel == BLUE));
        assert!(
            row[198..202]
                .iter()
                .any(|pixel| pixel[0] > 0 && pixel[2] > 0)
        );
        assert_eq!(row[400], background);
    }

    #[test]
    fn x11_element_damage_is_the_buffers_mapped_outward() {
        let mut scene = Scene::new(&[(400, 400, 1.0), (400, 400, 2.0)]);
        let mut damage = DamageBag::new(4);
        damage.add([Rectangle::from_size((1, 1).into())]);
        let before = damage.current_commit();
        damage.add([Rectangle::new((10, 10).into(), (3, 3).into())]);
        let image = Image::new(300, 300, noise);
        // On each output, X11 pixels [10, 13), widened by the texel a filter reaches, [9, 14),
        // mapped to pixels and rounded outward: at 2/3, through the area filter, [6, 9.33) is
        // [6, 10); at 4/3, through the bilinear filter, [12, 18.67) is [12, 19).
        for (output, rounded_out) in [(0, ((6, 6), (4, 4))), (1, ((12, 12), (7, 7)))] {
            let texture = scene.texture(&image, Fourcc::Xrgb8888);
            let surface = SurfaceTexture {
                id: Id::new(),
                texture,
                damage: damage.snapshot(),
            };
            let element = X11SurfaceElement::new(
                &mut scene.state.renderer,
                surface,
                Point::default(),
                scene.state.outputs[output].rect,
                1.5,
                1.0,
                &scene.state.outputs[output],
            )
            .expect("an element");
            let scale = Scale::from(scene.state.outputs[output].mode.scale());
            let damaged: Vec<_> = element
                .damage_since(scale, Some(before))
                .iter()
                .copied()
                .collect();
            assert_eq!(
                damaged,
                [Rectangle::new(rounded_out.0.into(), rounded_out.1.into())]
            );
            assert!(
                element
                    .damage_since(scale, Some(damage.current_commit()))
                    .is_empty()
            );
            let whole: Vec<_> = element.damage_since(scale, None).iter().copied().collect();
            assert_eq!(whole, [Rectangle::from_size(element.placement.shown.size)]);
        }
    }

    #[test]
    fn x11_element_redrawn_where_damaged_matches_a_whole_drawing() {
        // A buffer drawn whole, then again with a 3 × 3 block changed and only that damaged:
        // the pixels drawn again must be every pixel the change reaches, so that the frame
        // equals the new buffer drawn whole.
        let before = Image::new(300, 300, noise);
        let after = Image::new(300, 300, |x, y| {
            if (100..103).contains(&x) && (100..103).contains(&y) {
                WHITE
            } else {
                noise(x, y)
            }
        });
        let mut scene = Scene::new(&[(400, 400, 1.0), (400, 400, 2.0), (400, 400, 1.5)]);
        // The area filter at 2/3; the bilinear filter at 2, where a texel reaches a pixel past
        // its own two; the copy.
        for (output, x11_scale) in [(0, 1.5), (1, 1.0), (2, 1.5)] {
            let id = Id::new();
            let mut damage = DamageBag::new(4);
            damage.add([Rectangle::from_size((300, 300).into())]);
            let first = element_with(&mut scene, &id, &before, &damage, x11_scale, output);
            let shown = first.placement.shown;
            scene.draw_one(output, first);
            damage.add([Rectangle::new((100, 100).into(), (3, 3).into())]);
            let second = element_with(&mut scene, &id, &after, &damage, x11_scale, output);
            let elements = [WindowElement::X11(second)];
            scene.draw(output, &elements);
            let redrawn = scene.read(output, shown);
            scene.state.outputs[output].drawn_once = false;
            scene.draw(output, &elements);
            let whole = scene.read(output, shown);
            assert!(
                redrawn == whole,
                "output {output}: the damage missed pixels the change reaches"
            );
        }
    }

    /// The element of a surface `id` whose buffer is `image` and whose damage is `damage`'s,
    /// its window over the whole of output `output`.
    fn element_with(
        scene: &mut Scene,
        id: &Id,
        image: &Image,
        damage: &DamageBag<i32, Buffer>,
        x11_scale: f64,
        output: usize,
    ) -> X11SurfaceElement {
        let texture = scene.texture(image, Fourcc::Xrgb8888);
        let surface = SurfaceTexture {
            id: id.clone(),
            texture,
            damage: damage.snapshot(),
        };
        X11SurfaceElement::new(
            &mut scene.state.renderer,
            surface,
            Point::default(),
            scene.state.outputs[output].rect,
            x11_scale,
            1.0,
            &scene.state.outputs[output],
        )
        .expect("an element")
    }

    #[test]
    fn x11_element_falls_back_to_bilinear_when_the_area_filter_does_not_compile() {
        let mut scene = Scene::new(&[]);
        let renderer = &mut scene.state.renderer;
        assert!(compile_area_filter(renderer, AREA_FILTER).is_some());
        assert!(compile_area_filter(renderer, "#version 100\nnot a shader").is_none());
        // As if it had failed to compile: the renderer keeps the failure and draws bilinearly.
        renderer
            .egl_context()
            .user_data()
            .insert_if_missing(|| AreaFilterProgram(None));
        assert!(matches!(filter(renderer, 1.0, 1.5), Filter::Bilinear));
        assert!(matches!(filter(renderer, 1.5, 1.5), Filter::Copy));
    }

    /// A median, the 90th percentile and the longest of `times`, in microseconds.
    fn spread(mut times: Vec<Duration>) -> (u128, u128, u128) {
        times.sort();
        let at = |fraction: f64| times[((times.len() - 1) as f64 * fraction).round() as usize];
        (
            at(0.5).as_micros(),
            at(0.9).as_micros(),
            at(1.0).as_micros(),
        )
    }

    /// What drawing an X11 window costs on llvmpipe at 4K: the area filter against the
    /// renderer's bilinear filter and against the plain copy at XWayland's scale, each drawn
    /// whole 200 times after 10 to warm up. Run alone, on a quiet machine, and the figures go to
    /// `docs/measurements.md`:
    ///
    /// ```sh
    /// nix develop -c cargo test -p yuna --lib --locked x11_element_drawing_time -- --ignored --nocapture --test-threads=1
    /// ```
    #[test]
    #[ignore = "a measurement, run by hand on a quiet machine (docs/measurements.md)"]
    fn x11_element_drawing_time_at_4k() {
        const WARM_UP: usize = 10;
        const DRAWS: usize = 200;
        // Output 0 at 1, output 1 at 1.5, both 3840 × 2160; XWayland's scale is 1.5.
        let mut scene = Scene::new(&[(3840, 2160, 1.0), (3840, 2160, 1.5)]);
        // A window as wide as X11 sees output 0, which it covers; and one as wide as output 1.
        let wide = Image::new(5760, 3240, noise);
        let at_4k = Image::new(3840, 2160, noise);
        let mut series: Vec<(&str, usize, Vec<WindowElement>)> =
            vec![("the background alone, at 1", 0, Vec::new())];
        for (what, image, output, bilinear) in [
            ("5760 × 3240 to 3840 × 2160, area filter", &wide, 0, false),
            ("5760 × 3240 to 3840 × 2160, bilinear", &wide, 0, true),
            ("3840 × 2160 to 2560 × 1440, area filter", &at_4k, 0, false),
            ("3840 × 2160 to 2560 × 1440, bilinear", &at_4k, 0, true),
            ("3840 × 2160 copied at 1.5", &at_4k, 1, false),
        ] {
            let texture = scene.texture(image, Fourcc::Xrgb8888);
            let rect = scene.state.outputs[output].rect;
            let mut element = scene
                .element(texture, rect, 1.5, 1.0, output)
                .expect("an element");
            if bilinear {
                element.filter = Filter::Bilinear;
            }
            series.push((what, output, vec![WindowElement::X11(element)]));
        }
        for (what, output, elements) in &series {
            let mut times = Vec::with_capacity(DRAWS);
            for draw in 0..WARM_UP + DRAWS {
                // Drawn whole each time, as if nothing were kept from the frame before.
                scene.state.outputs[*output].drawn_once = false;
                let started = Instant::now();
                scene.draw(*output, elements);
                if draw >= WARM_UP {
                    times.push(started.elapsed());
                }
            }
            let (median, p90, longest) = spread(times);
            println!("{what}: median {median} µs, 90th percentile {p90} µs, longest {longest} µs");
        }
    }
}
