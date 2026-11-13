//! Captures: what an output, a region of it or a window shows, copied when the frame that shows
//! it is drawn ([ADR-0034](../../../docs/adr/0034-captures.md),
//! [ADR-0038](../../../docs/adr/0038-captures-coordinates-windows-and-wayland.md)).
//!
//! A capture asks an output's *newest* frame: the one its armed refresh draws, if a refresh is
//! armed, since a commit or a change of the layout is on its way to the screen; else the frame
//! its buffer holds, copied at once. A Wayland client's later frames of one session ask a *new*
//! frame instead: they wait for a refresh that draws something (`screencopy.rs`). Nothing is
//! drawn for a capture: a capture waiting on an output at rest waits until something changes.
//!
//! An agent's capture is an image whose pixels map to the output's logical units by its
//! [`CaptureMapping`]: at the logical size a pixel is a unit, at the physical size a pixel is
//! 1 / scale of one. The pixels are read here, on the main thread, at their frame; the image is
//! made from them by [`CapturedFrame::image`], which an encoder runs on a thread of its own, so
//! that no frame waits for it.
//!
//! The only part that reads pixels from the renderer is `render::read_pixels`.

use std::time::Duration;

use smithay::utils::{Physical, Rectangle};
use yuna_layout::geometry::Rect;
use yuna_layout::{OutputId, SeatId, WindowId};

use crate::Error;
use crate::clock::Deadline;
use crate::output::{OutputMode, to_i32};
use crate::screencopy::ClientFrame;
use crate::seats::seat_of_surface;
use crate::state::State;

/// What an agent captures.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureTarget {
    /// One of its seat's outputs, its first by default: all of it, or a region of it.
    Output {
        /// The output; the seat's first when none.
        output: Option<OutputId>,
        /// The region of it; all of it when none.
        region: Option<LogicalRegion>,
    },
    /// What its output shows where a window is, cut to the output
    /// ([ADR-0038](../../../docs/adr/0038-captures-coordinates-windows-and-wayland.md)).
    Window(WindowId),
}

/// The size of a capture's image.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum CaptureSize {
    /// A pixel for each logical unit: a pixel is where the agent clicks.
    #[default]
    Logical,
    /// A pixel for each of the output's physical pixels.
    Physical,
}

/// A rectangle of an output in whole logical units from its top-left corner.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LogicalRegion {
    /// Its left edge.
    pub x: u32,
    /// Its top edge.
    pub y: u32,
    /// Its width, at least 1.
    pub width: u32,
    /// Its height, at least 1.
    pub height: u32,
}

/// An agent's capture, until its pixels are copied.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CaptureTicket(pub u64);

/// How a capture's image maps to its output's logical coordinates: the image's point
/// (`px`, `py`), in pixels from its top-left corner, is the logical point
/// (`x + px × units_per_pixel`, `y + py × units_per_pixel`) of `output`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CaptureMapping {
    /// The output captured.
    pub output: OutputId,
    /// The logical point of the image's top-left corner, from the output's: across.
    pub x: f64,
    /// Down.
    pub y: f64,
    /// The logical units a pixel covers: 1 at the logical size, 1 / scale at the physical size.
    pub units_per_pixel: f64,
    /// The image's width in pixels.
    pub width: u32,
    /// Its height in pixels.
    pub height: u32,
}

impl CaptureMapping {
    /// The logical point of the image's point (`px`, `py`); none for a point off the image.
    pub fn to_logical(&self, px: f64, py: f64) -> Option<(f64, f64)> {
        let on = |p: f64, side: u32| p.is_finite() && (0.0..=f64::from(side)).contains(&p);
        (on(px, self.width) && on(py, self.height)).then_some((
            self.x + px * self.units_per_pixel,
            self.y + py * self.units_per_pixel,
        ))
    }
}

/// The pixels of an agent's capture, copied at the frame they show.
#[derive(Debug)]
pub struct CapturedFrame {
    /// How its image maps to the output's logical coordinates.
    pub mapping: CaptureMapping,
    /// The window captured, for a window's capture.
    pub window: Option<WindowId>,
    /// The refresh boundary the frame is presented at.
    pub boundary: u64,
    /// That boundary's time on the clock.
    pub presented_at: Duration,
    area: Area,
    /// The output's physical pixels per logical unit.
    scale: f64,
    /// `area.read`'s pixels, R, G, B, A, row by row from the top.
    pixels: Vec<u8>,
}

impl CapturedFrame {
    /// The image, `mapping.width` × `mapping.height` pixels of R, G, B, row by row from the
    /// top. At the logical size each pixel is the average of the physical pixels its logical
    /// unit covers, weighted by how much of each it covers.
    pub fn image(&self) -> Vec<u8> {
        match self.area.size {
            CaptureSize::Physical => self
                .pixels
                .as_chunks::<4>()
                .0
                .iter()
                .flat_map(|p| [p[0], p[1], p[2]])
                .collect(),
            CaptureSize::Logical => logical_image(&self.pixels, &self.area, self.scale),
        }
    }
}

/// What a capture reads, and how its image maps to logical coordinates.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Area {
    /// The output's physical pixels to read.
    read: Rectangle<i32, Physical>,
    mapping: CaptureMapping,
    size: CaptureSize,
}

/// A logical point within this of a whole unit counts as on it: a window's edges, on the
/// physical grid, are whole units only up to rounding.
const ON_THE_GRID: f64 = 1e-6;

/// The area of an output of `mode` that shows `rect`, logical units from the output's top-left
/// corner, cut to the output. At the logical size, `rect` widens to whole units; at the
/// physical size, to whole pixels. Refused when nothing of `rect` is on the output.
fn area(
    output: OutputId,
    mode: &OutputMode,
    rect: Rect,
    size: CaptureSize,
) -> Result<Area, String> {
    let scale = mode.scale();
    let (physical_width, physical_height) = (f64::from(mode.width()), f64::from(mode.height()));
    let cut = |start: f64, length: f64, side: f64| (start.max(0.0), (start + length).min(side));
    let (x0, x1) = cut(rect.x, rect.w, physical_width / scale);
    let (y0, y1) = cut(rect.y, rect.h, physical_height / scale);
    if x1 - x0 <= ON_THE_GRID || y1 - y0 <= ON_THE_GRID {
        return Err("nothing of it is on its output".to_owned());
    }
    let pixel = |v: f64| v.clamp(0.0, f64::from(i32::MAX)) as i32;
    match size {
        CaptureSize::Logical => {
            let (lx0, ly0) = ((x0 + ON_THE_GRID).floor(), (y0 + ON_THE_GRID).floor());
            let (lx1, ly1) = ((x1 - ON_THE_GRID).ceil(), (y1 - ON_THE_GRID).ceil());
            let read = Rectangle::from_extremities(
                (
                    pixel((lx0 * scale + ON_THE_GRID).floor()),
                    pixel((ly0 * scale + ON_THE_GRID).floor()),
                ),
                (
                    pixel((lx1 * scale - ON_THE_GRID).ceil().min(physical_width)),
                    pixel((ly1 * scale - ON_THE_GRID).ceil().min(physical_height)),
                ),
            );
            Ok(Area {
                read,
                mapping: CaptureMapping {
                    output,
                    x: lx0,
                    y: ly0,
                    units_per_pixel: 1.0,
                    width: (lx1 - lx0) as u32,
                    height: (ly1 - ly0) as u32,
                },
                size,
            })
        }
        CaptureSize::Physical => {
            let (px0, py0) = (
                (x0 * scale + ON_THE_GRID).floor(),
                (y0 * scale + ON_THE_GRID).floor(),
            );
            let (px1, py1) = (
                (x1 * scale - ON_THE_GRID).ceil().min(physical_width),
                (y1 * scale - ON_THE_GRID).ceil().min(physical_height),
            );
            Ok(Area {
                read: Rectangle::from_extremities(
                    (pixel(px0), pixel(py0)),
                    (pixel(px1), pixel(py1)),
                ),
                mapping: CaptureMapping {
                    output,
                    x: px0 / scale,
                    y: py0 / scale,
                    units_per_pixel: 1.0 / scale,
                    width: (px1 - px0) as u32,
                    height: (py1 - py0) as u32,
                },
                size,
            })
        }
    }
}

/// An output's logical size in whole units, its last unit counted when the physical size does
/// not divide by the scale.
fn logical_units(mode: &OutputMode) -> (u32, u32) {
    let units = |pixels: u32| (f64::from(pixels) / mode.scale() - ON_THE_GRID).ceil() as u32;
    (units(mode.width()), units(mode.height()))
}

/// Which physical pixels each pixel of a logical image takes, along one axis, and with which
/// weights: pixel `i` covers the logical units [`origin` + i, `origin` + i + 1), that is the
/// physical span [(`origin` + i) × `scale`, …) from the output's edge, `read_start` of which
/// were not read. Weights are each pixel's share of the span, cut to the `read_length` pixels
/// read, and add up to 1.
fn axis_taps(
    origin: f64,
    count: u32,
    scale: f64,
    read_start: i32,
    read_length: usize,
) -> Vec<(usize, Vec<f32>)> {
    (0..count)
        .map(|i| {
            let start = ((origin + f64::from(i)) * scale - f64::from(read_start)).max(0.0);
            let end = ((origin + f64::from(i) + 1.0) * scale - f64::from(read_start))
                .min(read_length as f64);
            let first = (start.floor() as usize).min(read_length.saturating_sub(1));
            let mut weights = Vec::new();
            let mut j = first;
            while (j as f64) < end && j < read_length {
                let covered = end.min(j as f64 + 1.0) - start.max(j as f64);
                weights.push(covered.max(0.0) as f32);
                j += 1;
            }
            let total: f32 = weights.iter().sum();
            if total > 0.0 {
                weights.iter_mut().for_each(|w| *w /= total);
            } else {
                // A span narrower than rounding: the pixel it starts in.
                weights = vec![1.0];
            }
            (first, weights)
        })
        .collect()
}

/// The logical image of `pixels`, the R, G, B, A pixels `area.read` covers: each image pixel
/// the weighted average of the physical pixels under it, across then down.
fn logical_image(pixels: &[u8], area: &Area, scale: f64) -> Vec<u8> {
    let (read_width, read_height) = (
        area.read.size.w.max(0) as usize,
        area.read.size.h.max(0) as usize,
    );
    let mapping = &area.mapping;
    let columns = axis_taps(mapping.x, mapping.width, scale, area.read.loc.x, read_width);
    let rows = axis_taps(
        mapping.y,
        mapping.height,
        scale,
        area.read.loc.y,
        read_height,
    );
    let width = mapping.width as usize;
    // Physical rows made narrow, across, are kept while the next logical rows may take them
    // again: a logical row takes as many as its taps, and shares its first with the row before.
    let keep = rows
        .iter()
        .map(|(_, weights)| weights.len())
        .max()
        .unwrap_or(1)
        + 1;
    let mut narrow_rows: Vec<(usize, Vec<f32>)> = Vec::with_capacity(keep);
    let mut image = Vec::with_capacity(width * mapping.height as usize * 3);
    let mut sum = vec![0.0f32; width * 3];
    for (first, weights) in &rows {
        for row in *first..first + weights.len() {
            if narrow_rows.iter().any(|(kept, _)| *kept == row) {
                continue;
            }
            let mut out = if narrow_rows.len() >= keep {
                narrow_rows.remove(0).1
            } else {
                vec![0.0f32; width * 3]
            };
            narrow_row(
                &pixels[row * read_width * 4..(row + 1) * read_width * 4],
                &columns,
                &mut out,
            );
            narrow_rows.push((row, out));
        }
        sum.fill(0.0);
        for (k, weight) in weights.iter().enumerate() {
            if let Some((_, values)) = narrow_rows.iter().find(|(kept, _)| *kept == first + k) {
                for (total, value) in sum.iter_mut().zip(values) {
                    *total += weight * value;
                }
            }
        }
        image.extend(sum.iter().map(|v| v.round().clamp(0.0, 255.0) as u8));
    }
    image
}

/// Writes into `out` the R, G, B of one physical row of R, G, B, A, `source`, made narrow by
/// `columns`' taps.
fn narrow_row(source: &[u8], columns: &[(usize, Vec<f32>)], out: &mut [f32]) {
    out.fill(0.0);
    for ((first, weights), pixel) in columns.iter().zip(out.as_chunks_mut::<3>().0) {
        for (k, weight) in weights.iter().enumerate() {
            let p = &source[(first + k) * 4..(first + k) * 4 + 3];
            for c in 0..3 {
                pixel[c] += weight * f32::from(p[c]);
            }
        }
    }
}

/// Where a capture's pixels go once copied.
pub(crate) enum Destination {
    /// An agent's capture, through MCP.
    Agent {
        ticket: CaptureTicket,
        target: CaptureTarget,
        size: CaptureSize,
    },
    /// A Wayland client's frame (`screencopy.rs`).
    Client(ClientFrame),
}

/// A capture waiting for its output's refresh.
pub(crate) struct PendingCapture {
    pub output: OutputId,
    /// Whether it waits for a refresh that draws something, rather than the next refresh.
    pub new_content: bool,
    pub destination: Destination,
}

impl State {
    /// Asks a capture of `target` on agent seat `seat`'s output, at `size`. Refused, with what
    /// to do, for another seat's output or window, a hidden window, and a region off its
    /// output. Its pixels come in [`State::take_captures`], at its output's newest frame.
    pub(crate) fn capture(
        &mut self,
        seat: SeatId,
        target: CaptureTarget,
        size: CaptureSize,
    ) -> Result<CaptureTicket, Error> {
        let output = match target {
            CaptureTarget::Output { output, region } => {
                let output = match output {
                    Some(output) => output,
                    None => self
                        .outputs
                        .iter()
                        .find(|o| o.seat == seat)
                        .map(|o| o.id)
                        .ok_or_else(|| Error::Refused(format!("seat {} has no output", seat.0)))?,
                };
                let index = self
                    .output_index(output)
                    .filter(|&index| self.outputs[index].seat == seat)
                    .ok_or_else(|| {
                        Error::Refused(format!("no output {} on seat {}", output.0, seat.0))
                    })?;
                if let Some(region) = region {
                    let (width, height) = logical_units(&self.outputs[index].mode);
                    let fits = |start: u32, length: u32, side: u32| {
                        length > 0 && u64::from(start) + u64::from(length) <= u64::from(side)
                    };
                    if !fits(region.x, region.width, width)
                        || !fits(region.y, region.height, height)
                    {
                        return Err(Error::Refused(format!(
                            "the region is not wholly on output {}, {width} × {height} logical units",
                            output.0
                        )));
                    }
                }
                output
            }
            CaptureTarget::Window(window) => {
                let toplevel = self
                    .windows
                    .get(&window)
                    .and_then(|id| self.toplevels.get(id))
                    .filter(|toplevel| seat_of_surface(toplevel.wl_surface()) == seat)
                    .ok_or_else(|| {
                        Error::Refused(format!("no window {} on seat {}", window.0, seat.0))
                    })?;
                toplevel.shown_on.ok_or_else(|| {
                    Error::Refused(format!(
                        "window {} is hidden: show its workspace with `workspaces focus`, or \
                         scroll it into view",
                        window.0
                    ))
                })?
            }
        };
        let ticket = CaptureTicket(self.next_capture_ticket);
        self.next_capture_ticket += 1;
        self.queue_capture(
            output,
            false,
            Destination::Agent {
                ticket,
                target,
                size,
            },
        );
        Ok(ticket)
    }

    /// The agents' captures copied since the last call, or failed, with why.
    pub(crate) fn take_captures(&mut self) -> Vec<(CaptureTicket, Result<CapturedFrame, String>)> {
        std::mem::take(&mut self.captured)
    }

    /// Drops an agent's capture that waits for its frame; nothing if it was copied already.
    pub(crate) fn cancel_capture(&mut self, ticket: CaptureTicket) {
        self.pending_captures.retain(|pending| {
            !matches!(pending.destination, Destination::Agent { ticket: t, .. } if t == ticket)
        });
    }

    /// Queues a capture of `output`: copied now, from the frame its buffer holds, when it waits
    /// for no new content and no refresh is armed; else at the next refresh, or the next that
    /// draws something when `new_content`.
    pub(crate) fn queue_capture(
        &mut self,
        output: OutputId,
        new_content: bool,
        destination: Destination,
    ) {
        let Some(index) = self.output_index(output) else {
            fail(&mut self.captured, destination, "its output was removed");
            return;
        };
        if !new_content && !self.clock.is_armed(Deadline::Refresh(output)) {
            let seq = self.outputs[index].last_refresh;
            self.copy(index, vec![destination], seq);
        } else {
            self.pending_captures.push(PendingCapture {
                output,
                new_content,
                destination,
            });
        }
    }

    /// At output `id`'s refresh, presented at boundary `seq`: copies the captures that waited
    /// for it, those that wait for new content only if it `drew`.
    pub(crate) fn copy_captures(&mut self, id: OutputId, drew: bool, seq: u64) {
        if self.pending_captures.is_empty() {
            return;
        }
        let (due, waiting): (Vec<PendingCapture>, Vec<PendingCapture>) =
            std::mem::take(&mut self.pending_captures)
                .into_iter()
                .partition(|pending| pending.output == id && (drew || !pending.new_content));
        self.pending_captures = waiting;
        if let Some(index) = self.output_index(id) {
            self.copy(
                index,
                due.into_iter().map(|pending| pending.destination).collect(),
                seq,
            );
        }
    }

    /// Fails every capture that waits on output `id`, saying `why`.
    pub(crate) fn fail_captures(&mut self, id: OutputId, why: &str) {
        let (failed, waiting): (Vec<PendingCapture>, Vec<PendingCapture>) =
            std::mem::take(&mut self.pending_captures)
                .into_iter()
                .partition(|pending| pending.output == id);
        self.pending_captures = waiting;
        for pending in failed {
            fail(&mut self.captured, pending.destination, why);
        }
    }

    /// Copies output `index`'s last frame, presented at boundary `seq`, for each destination:
    /// the region each agent's capture shows, and the whole output once for every client's.
    fn copy(&mut self, index: usize, destinations: Vec<Destination>, seq: u64) {
        let output = &self.outputs[index];
        let presented_at = output.boundaries.at(seq);
        if !output.drawn_once {
            for destination in destinations {
                fail(
                    &mut self.captured,
                    destination,
                    "its output has drawn no frame yet",
                );
            }
            return;
        }
        let (id, mode) = (output.id, output.mode);
        let whole = Rectangle::from_size((to_i32(mode.width()), to_i32(mode.height())).into());
        let mut whole_pixels: Option<Result<Vec<u8>, String>> = None;
        for destination in destinations {
            match destination {
                Destination::Agent {
                    ticket,
                    target,
                    size,
                } => {
                    let copied = self
                        .agent_area(index, target, size)
                        .and_then(|(area, window)| {
                            let pixels = crate::render::read_pixels(
                                &mut self.renderer,
                                &mut self.outputs[index],
                                area.read,
                            )
                            .map_err(|e| e.to_string())?;
                            Ok(CapturedFrame {
                                mapping: area.mapping,
                                window,
                                boundary: seq,
                                presented_at,
                                area,
                                scale: mode.scale(),
                                pixels,
                            })
                        });
                    self.captured.push((ticket, copied));
                }
                Destination::Client(frame) => {
                    let pixels = whole_pixels.get_or_insert_with(|| {
                        crate::render::read_pixels(
                            &mut self.renderer,
                            &mut self.outputs[index],
                            whole,
                        )
                        .map_err(|e| e.to_string())
                    });
                    match pixels {
                        Ok(pixels) => self.finish_client_frame(
                            frame,
                            pixels,
                            mode.width(),
                            mode.height(),
                            presented_at,
                        ),
                        Err(error) => {
                            tracing::warn!(%error, output = id.0, "a client's capture failed");
                            frame.fail();
                        }
                    }
                }
            }
        }
    }

    /// What an agent's capture of `target` reads on output `index`, as the output last arranged
    /// it, and the window it shows.
    fn agent_area(
        &self,
        index: usize,
        target: CaptureTarget,
        size: CaptureSize,
    ) -> Result<(Area, Option<WindowId>), String> {
        let output = &self.outputs[index];
        match target {
            CaptureTarget::Output { region, .. } => {
                let rect = match region {
                    Some(r) => Rect::new(
                        f64::from(r.x),
                        f64::from(r.y),
                        f64::from(r.width),
                        f64::from(r.height),
                    ),
                    None => Rect::new(0.0, 0.0, output.rect.w, output.rect.h),
                };
                area(output.id, &output.mode, rect, size).map(|area| (area, None))
            }
            CaptureTarget::Window(window) => {
                let placed = output
                    .placed
                    .iter()
                    .find(|placed| placed.window == window && placed.rect.overlaps(&output.rect))
                    .ok_or_else(|| {
                        format!(
                            "window {} left output {} before its frame: capture it again",
                            window.0, output.id.0
                        )
                    })?;
                let rect = Rect::new(
                    placed.rect.x - output.rect.x,
                    placed.rect.y - output.rect.y,
                    placed.rect.w,
                    placed.rect.h,
                );
                area(output.id, &output.mode, rect, size)
                    .map(|area| (area, Some(window)))
                    .map_err(|why| format!("window {}: {why}", window.0))
            }
        }
    }
}

/// Fails a capture that will not be copied: an agent's with `why`, a client's as stopped.
fn fail(
    captured: &mut Vec<(CaptureTicket, Result<CapturedFrame, String>)>,
    destination: Destination,
    why: &str,
) {
    match destination {
        Destination::Agent { ticket, .. } => captured.push((ticket, Err(why.to_owned()))),
        Destination::Client(frame) => frame.fail(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const OUTPUT: OutputId = OutputId(1);

    fn default_mode() -> OutputMode {
        OutputMode::default()
    }

    #[test]
    fn a_whole_output_at_the_logical_size_maps_a_pixel_to_a_unit() {
        let mode = default_mode();
        let area = area(
            OUTPUT,
            &mode,
            Rect::new(0.0, 0.0, 2560.0, 1440.0),
            CaptureSize::Logical,
        )
        .expect("an area");
        assert_eq!(area.read, Rectangle::from_size((3840, 2160).into()));
        assert_eq!(
            area.mapping,
            CaptureMapping {
                output: OUTPUT,
                x: 0.0,
                y: 0.0,
                units_per_pixel: 1.0,
                width: 2560,
                height: 1440
            }
        );
        assert_eq!(area.mapping.to_logical(100.0, 50.5), Some((100.0, 50.5)));
        assert_eq!(area.mapping.to_logical(2561.0, 0.0), None);
        assert_eq!(area.mapping.to_logical(f64::NAN, 0.0), None);
    }

    #[test]
    fn a_region_at_the_physical_size_maps_a_pixel_to_a_part_of_a_unit() {
        let mode = default_mode();
        let area = area(
            OUTPUT,
            &mode,
            Rect::new(10.0, 20.0, 100.0, 50.0),
            CaptureSize::Physical,
        )
        .expect("an area");
        assert_eq!(area.read, Rectangle::new((15, 30).into(), (150, 75).into()));
        assert_eq!((area.mapping.width, area.mapping.height), (150, 75));
        assert_eq!((area.mapping.x, area.mapping.y), (10.0, 20.0));
        let (x, y) = area.mapping.to_logical(30.0, 15.0).expect("on the image");
        assert!(
            (x - 30.0).abs() < 1e-9 && (y - 30.0).abs() < 1e-9,
            "{x} {y}"
        );
    }

    #[test]
    fn a_window_off_the_grid_widens_to_whole_units_and_is_cut_to_its_output() {
        let mode = default_mode();
        // 20 pixels from the left edge, 13.33 units; its right part off the output.
        let rect = Rect::new(20.0 / 1.5, 16.0, 3000.0, 100.0);
        let logical = area(OUTPUT, &mode, rect, CaptureSize::Logical).expect("an area");
        assert_eq!(
            (logical.mapping.x, logical.mapping.width),
            (13.0, 2560 - 13)
        );
        assert_eq!(logical.read.loc.x, 19);
        assert_eq!(logical.read.loc.x + logical.read.size.w, 3840);
        let physical = area(OUTPUT, &mode, rect, CaptureSize::Physical).expect("an area");
        assert_eq!(physical.read.loc.x, 20);
        assert_eq!(physical.mapping.width, 3820);
        assert!((physical.mapping.x - 20.0 / 1.5).abs() < 1e-9);
        assert!(
            area(
                OUTPUT,
                &mode,
                Rect::new(2600.0, 0.0, 10.0, 10.0),
                CaptureSize::Logical
            )
            .is_err()
        );
    }

    #[test]
    fn an_output_whose_size_does_not_divide_by_its_scale_counts_its_last_unit() {
        let mode = OutputMode::new(1366, 768, 60.0, 1.5).expect("a mode");
        assert_eq!(logical_units(&mode), (911, 512));
        let area = area(
            OUTPUT,
            &mode,
            Rect::new(0.0, 0.0, 1366.0 / 1.5, 512.0),
            CaptureSize::Logical,
        )
        .expect("an area");
        assert_eq!((area.mapping.width, area.mapping.height), (911, 512));
        assert_eq!(area.read, Rectangle::from_size((1366, 768).into()));
        assert_eq!(logical_units(&default_mode()), (2560, 1440));
    }

    fn captured(pixels: Vec<u8>, area: Area, scale: f64) -> CapturedFrame {
        CapturedFrame {
            mapping: area.mapping,
            window: None,
            boundary: 0,
            presented_at: Duration::ZERO,
            area,
            scale,
            pixels,
        }
    }

    #[test]
    fn a_logical_pixel_averages_the_physical_pixels_it_covers() {
        // Scale 2: each logical pixel is a 2 × 2 block; here white and black columns.
        let mode = OutputMode::new(4, 2, 60.0, 2.0).expect("a mode");
        let area = area(
            OUTPUT,
            &mode,
            Rect::new(0.0, 0.0, 2.0, 1.0),
            CaptureSize::Logical,
        )
        .expect("an area");
        let row = [
            255u8, 255, 255, 255, 0, 0, 0, 255, 255, 0, 0, 255, 255, 0, 0, 255,
        ];
        let pixels = [row, row].concat();
        let image = captured(pixels, area, 2.0).image();
        assert_eq!(image, vec![128, 128, 128, 255, 0, 0]);
    }

    #[test]
    fn at_scale_one_and_a_half_a_pixel_shared_by_two_units_counts_for_both() {
        // 3 physical pixels across, 2 logical units: the middle one is half in each.
        let mode = OutputMode::new(3, 3, 60.0, 1.5).expect("a mode");
        let area = area(
            OUTPUT,
            &mode,
            Rect::new(0.0, 0.0, 2.0, 2.0),
            CaptureSize::Logical,
        )
        .expect("an area");
        let row = [0u8, 0, 0, 255, 90, 90, 90, 255, 180, 180, 180, 255];
        let pixels = [row, row, row].concat();
        let image = captured(pixels, area, 1.5).image();
        // (0 + 0.5 × 90) / 1.5 = 30; (0.5 × 90 + 180) / 1.5 = 150.
        assert_eq!(image, [[30u8, 30, 30, 150, 150, 150]; 2].concat());
    }

    #[test]
    fn a_physical_image_keeps_each_pixel_without_its_alpha() {
        let mode = OutputMode::new(2, 1, 60.0, 1.0).expect("a mode");
        let area = area(
            OUTPUT,
            &mode,
            Rect::new(0.0, 0.0, 2.0, 1.0),
            CaptureSize::Physical,
        )
        .expect("an area");
        let image = captured(vec![1, 2, 3, 255, 4, 5, 6, 255], area, 1.0).image();
        assert_eq!(image, vec![1, 2, 3, 4, 5, 6]);
    }

    #[test]
    fn taps_add_up_to_one_and_stay_in_what_was_read() {
        for scale in [0.25, 1.0, 1.25, 1.5, 2.0, 3.0, 8.0] {
            let taps = axis_taps(
                3.0,
                17,
                scale,
                (3.0 * scale) as i32,
                (20.0 * scale).ceil() as usize,
            );
            for (first, weights) in &taps {
                let total: f32 = weights.iter().sum();
                assert!((total - 1.0).abs() < 1e-4, "{scale}: {total}");
                assert!(
                    first + weights.len() <= (20.0 * scale).ceil() as usize,
                    "{scale}"
                );
            }
        }
    }
}
