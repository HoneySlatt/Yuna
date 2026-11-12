//! Virtual outputs: outputs of any size, rate and scale that no monitor shows, drawn offscreen
//! ([ADR-0022](../../../docs/adr/0022-the-headless-backend-in-yuna.md)), and their refresh
//! boundaries ([ADR-0023](../../../docs/adr/0023-frames-presentation-and-the-clock.md)).

use std::fmt;
use std::time::Duration;

use smithay::backend::renderer::damage::OutputDamageTracker;
use smithay::backend::renderer::gles::GlesRenderbuffer;
use smithay::output::{Mode, Output, PhysicalProperties, Scale, Subpixel};
use smithay::utils::Transform;
use yuna_layout::geometry::{Point, Rect};
use yuna_layout::{OutputConfig, OutputId, Placed};

use crate::Error;
use crate::state::{OWNER_SEAT, State};

/// The slowest refresh rate accepted, in Hz.
const MIN_RATE: f64 = 1.0;
/// The fastest refresh rate accepted, in Hz.
const MAX_RATE: f64 = 1_000.0;
/// The smallest scale accepted.
const MIN_SCALE: f64 = 0.25;
/// The largest scale accepted.
const MAX_SCALE: f64 = 8.0;
/// Picoseconds in a second over millihertz: a refresh period in nanoseconds is this over the
/// rate in millihertz.
const NANOS_PER_SECOND_IN_MILLIHERTZ: u128 = 1_000_000_000_000;

/// A virtual output's size, refresh rate and scale, checked by [`OutputMode::new`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OutputMode {
    width: u32,
    height: u32,
    refresh_millihertz: u32,
    scale: f64,
}

/// Why an output's mode was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvalidOutput(pub String);

impl fmt::Display for InvalidOutput {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for InvalidOutput {}

impl OutputMode {
    /// A mode of `width` × `height` physical pixels, refreshed `rate` times a second, at
    /// `scale` physical pixels per logical unit. Refused unless both sides are positive, the
    /// rate between 1 and 1,000 Hz and the scale between 0.25 and 8. The renderer's own limit
    /// on sizes is checked when the output is created.
    pub fn new(
        width: u32,
        height: u32,
        rate: f64,
        scale: f64,
    ) -> Result<OutputMode, InvalidOutput> {
        if width == 0 || height == 0 {
            return Err(InvalidOutput(format!(
                "an output of {width} × {height} pixels has no area"
            )));
        }
        if !(MIN_RATE..=MAX_RATE).contains(&rate) {
            return Err(InvalidOutput(format!(
                "a refresh rate of {rate} Hz is outside {MIN_RATE}–{MAX_RATE} Hz"
            )));
        }
        if !(MIN_SCALE..=MAX_SCALE).contains(&scale) {
            return Err(InvalidOutput(format!(
                "a scale of {scale} is outside {MIN_SCALE}–{MAX_SCALE}"
            )));
        }
        // Within 1–1,000 Hz, the rate in millihertz fits a u32 and is at least 1,000.
        let refresh_millihertz = (rate * 1_000.0).round() as u32;
        Ok(OutputMode {
            width,
            height,
            refresh_millihertz,
            scale,
        })
    }

    /// Its width in physical pixels.
    pub fn width(&self) -> u32 {
        self.width
    }

    /// Its height in physical pixels.
    pub fn height(&self) -> u32 {
        self.height
    }

    /// Its refresh rate in millihertz, as Wayland gives it.
    pub fn refresh_millihertz(&self) -> u32 {
        self.refresh_millihertz
    }

    /// Its scale: physical pixels per logical unit.
    pub fn scale(&self) -> f64 {
        self.scale
    }

    /// The time between two refresh boundaries, to the nanosecond below.
    pub fn period(&self) -> Duration {
        let nanos = NANOS_PER_SECOND_IN_MILLIHERTZ / u128::from(self.refresh_millihertz);
        Duration::from_nanos(u64::try_from(nanos).unwrap_or(u64::MAX))
    }
}

impl Default for OutputMode {
    /// 3840 × 2160 at 240 Hz and scale 1.5: an agent's output (ADR-0015, ADR-0022).
    fn default() -> OutputMode {
        OutputMode {
            width: 3840,
            height: 2160,
            refresh_millihertz: 240_000,
            scale: 1.5,
        }
    }
}

/// A virtual output as the compositor holds it.
#[derive(Debug)]
pub(crate) struct VirtualOutput {
    pub id: OutputId,
    /// Smithay's output: what `wl_output` and `xdg-output` tell clients.
    pub output: Output,
    pub mode: OutputMode,
    /// Its place in the layout's logical coordinates.
    pub rect: Rect,
    /// What it is drawn into: `Abgr8888` at its physical size, its format its own, so that an
    /// output can later be HDR (ADR-0022).
    pub buffer: GlesRenderbuffer,
    pub damage: OutputDamageTracker,
    /// Whether `buffer` holds the last frame drawn, so that only damage needs drawing.
    pub drawn_once: bool,
    /// Its refresh boundaries, from its creation.
    pub boundaries: Boundaries,
    /// The boundary its armed refresh deadline is at.
    pub armed_boundary: u64,
    /// The boundary its last frame drawn was presented at.
    pub last_presented: Option<u64>,
    /// The windows it shows, bottom to top, as the layout last arranged them.
    pub placed: Vec<Placed>,
}

impl State {
    /// Creates a virtual output to the right of the others, on the owner's seat, and arms its
    /// first frame. Refused, with nothing created, for a side beyond the renderer's
    /// `GL_MAX_RENDERBUFFER_SIZE`.
    pub(crate) fn add_output(&mut self, mode: OutputMode) -> Result<OutputId, Error> {
        let limit = self.max_output_side;
        if mode.width() > limit || mode.height() > limit {
            return Err(Error::InvalidOutput(InvalidOutput(format!(
                "an output of {} × {} pixels is larger than the renderer's limit of {limit}",
                mode.width(),
                mode.height()
            ))));
        }
        let buffer = crate::render::create_buffer(&mut self.renderer, mode.width(), mode.height())?;
        let id = self.take_output_id();
        let right_edge = self
            .outputs
            .iter()
            .map(|o| o.rect.right())
            .fold(0.0, f64::max);
        let config = OutputConfig {
            position: Point {
                x: right_edge,
                y: 0.0,
            },
            physical_width: mode.width(),
            physical_height: mode.height(),
            scale: mode.scale(),
        };
        self.layout
            .add_output(OWNER_SEAT, id, config)
            .map_err(|e| {
                Error::InvalidOutput(InvalidOutput(format!("the layout refuses it: {e}")))
            })?;
        let rect = self
            .layout
            .output_rect(id)
            .map_err(|e| Error::Layout(e.to_string()))?;
        let output = Output::new(
            format!("HEADLESS-{}", id.0),
            PhysicalProperties {
                size: (0, 0).into(),
                subpixel: Subpixel::Unknown,
                make: "Yuna".to_owned(),
                model: "Virtual output".to_owned(),
                serial_number: id.0.to_string(),
            },
        );
        let smithay_mode = Mode {
            size: (to_i32(mode.width()), to_i32(mode.height())).into(),
            refresh: to_i32(mode.refresh_millihertz()),
        };
        output.change_current_state(
            Some(smithay_mode),
            Some(Transform::Normal),
            Some(Scale::Fractional(mode.scale())),
            Some((rect.x.round() as i32, rect.y.round() as i32).into()),
        );
        output.set_preferred(smithay_mode);
        // The global lives in the display: dropping its id does not remove it.
        output.create_global::<State>(&self.display_handle);
        let damage = OutputDamageTracker::from_output(&output);
        let boundaries = Boundaries {
            created_at: self.clock.now(),
            refresh_millihertz: mode.refresh_millihertz(),
        };
        self.outputs.push(VirtualOutput {
            id,
            output,
            mode,
            rect,
            buffer,
            damage,
            drawn_once: false,
            boundaries,
            armed_boundary: 0,
            last_presented: None,
            placed: Vec::new(),
        });
        tracing::info!(
            output = id.0,
            width = mode.width(),
            height = mode.height(),
            millihertz = mode.refresh_millihertz(),
            scale = mode.scale(),
            "virtual output created"
        );
        self.mark_layout_changed();
        Ok(id)
    }
}

/// A side or a rate as Smithay takes it, as an `i32`: sides are checked against the renderer's
/// limit, and rates against 1,000 Hz, far below `i32::MAX`, before they get here.
pub(crate) fn to_i32(value: u32) -> i32 {
    i32::try_from(value).unwrap_or(i32::MAX)
}

/// An output's refresh boundaries: boundary 0 is its creation, and boundary `k` comes `k`
/// periods later, to the nanosecond below, computed from the rate so that errors never add up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Boundaries {
    pub created_at: Duration,
    pub refresh_millihertz: u32,
}

impl Boundaries {
    /// The time of boundary `k`.
    pub fn at(&self, k: u64) -> Duration {
        let nanos =
            u128::from(k) * NANOS_PER_SECOND_IN_MILLIHERTZ / u128::from(self.refresh_millihertz);
        self.created_at.saturating_add(Duration::from_nanos(
            u64::try_from(nanos).unwrap_or(u64::MAX),
        ))
    }

    /// The first boundary at `time` or after it.
    pub fn first_at_or_after(&self, time: Duration) -> u64 {
        let elapsed = time.saturating_sub(self.created_at).as_nanos();
        let estimate = (elapsed * u128::from(self.refresh_millihertz))
            .div_ceil(NANOS_PER_SECOND_IN_MILLIHERTZ);
        let mut k = u64::try_from(estimate).unwrap_or(u64::MAX);
        // `at` rounds down: the estimate may be one off either way.
        while k > 0 && self.at(k - 1) >= time {
            k -= 1;
        }
        while self.at(k) < time {
            k += 1;
        }
        k
    }

    /// The first boundary strictly after `time`.
    pub fn first_after(&self, time: Duration) -> u64 {
        let k = self.first_at_or_after(time);
        if self.at(k) == time { k + 1 } else { k }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modes_out_of_range_are_refused() {
        assert!(OutputMode::new(0, 1080, 60.0, 1.0).is_err());
        assert!(OutputMode::new(1920, 0, 60.0, 1.0).is_err());
        assert!(OutputMode::new(1920, 1080, 0.5, 1.0).is_err());
        assert!(OutputMode::new(1920, 1080, 1_000.5, 1.0).is_err());
        assert!(OutputMode::new(1920, 1080, f64::NAN, 1.0).is_err());
        assert!(OutputMode::new(1920, 1080, 60.0, 0.2).is_err());
        assert!(OutputMode::new(1920, 1080, 60.0, 8.5).is_err());
        assert!(OutputMode::new(1920, 1080, 60.0, f64::INFINITY).is_err());
        assert!(OutputMode::new(1, 1, 1.0, 0.25).is_ok());
        assert!(OutputMode::new(1920, 1080, 1_000.0, 8.0).is_ok());
    }

    #[test]
    fn a_rate_is_kept_in_millihertz() {
        let mode = OutputMode::new(1920, 1080, 59.94, 1.0).expect("a valid mode");
        assert_eq!(mode.refresh_millihertz(), 59_940);
        assert_eq!(OutputMode::default().refresh_millihertz(), 240_000);
        assert_eq!(
            OutputMode::default().period(),
            Duration::from_nanos(4_166_666)
        );
    }

    fn boundaries_at_240_hz() -> Boundaries {
        Boundaries {
            created_at: Duration::from_secs(1_000),
            refresh_millihertz: 240_000,
        }
    }

    #[test]
    fn boundaries_are_the_creation_plus_whole_periods() {
        let b = boundaries_at_240_hz();
        assert_eq!(b.at(0), Duration::from_secs(1_000));
        assert_eq!(
            b.at(1),
            Duration::from_secs(1_000) + Duration::from_nanos(4_166_666)
        );
        assert_eq!(
            b.at(3),
            Duration::from_secs(1_000) + Duration::from_nanos(12_500_000)
        );
        // A second of boundaries ends exactly on the second: errors never add up.
        assert_eq!(b.at(240), Duration::from_secs(1_001));
        assert_eq!(b.at(240 * 3_600), Duration::from_secs(4_600));
    }

    #[test]
    fn the_boundary_at_or_after_a_time_and_strictly_after_it() {
        let b = boundaries_at_240_hz();
        let start = b.at(0);
        assert_eq!(b.first_at_or_after(start), 0);
        assert_eq!(b.first_after(start), 1);
        assert_eq!(b.first_at_or_after(b.at(1)), 1);
        assert_eq!(b.first_after(b.at(1)), 2);
        assert_eq!(b.first_at_or_after(b.at(1) + Duration::from_nanos(1)), 2);
        assert_eq!(b.first_at_or_after(b.at(1) - Duration::from_nanos(1)), 1);
        // Before the output existed, its first boundary is its creation.
        assert_eq!(b.first_at_or_after(start - Duration::from_secs(1)), 0);
        for k in [7, 239, 240, 241, 100_000] {
            assert_eq!(b.first_at_or_after(b.at(k)), k);
            assert_eq!(b.first_after(b.at(k)), k + 1);
            assert_eq!(b.first_after(b.at(k) - Duration::from_nanos(1)), k);
        }
    }

    #[test]
    fn boundaries_at_a_rate_that_does_not_divide_a_second() {
        let b = Boundaries {
            created_at: Duration::ZERO,
            refresh_millihertz: 59_940,
        };
        for k in [1, 2, 59, 60, 1_000, 1_000_000] {
            assert_eq!(b.first_at_or_after(b.at(k)), k);
            assert!(b.at(k) > b.at(k - 1));
        }
    }
}
