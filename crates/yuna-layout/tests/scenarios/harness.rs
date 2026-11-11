//! A model under test: every operation is unwrapped and followed by a check of every invariant,
//! so that a scenario fails at the step that breaks one.

use yuna_layout::geometry::{Point, Rect};
use yuna_layout::{
    Layout, LayoutError, OpenOptions, Options, OutputConfig, OutputId, Placed, SeatId, WindowId,
    WindowInfo,
};

/// The owner's seat.
pub const OWNER: SeatId = SeatId(1);
/// An agent's seat.
pub const AGENT: SeatId = SeatId(2);
/// An output of 1920 × 1080 at scale 1, at the origin.
pub const MAIN: OutputId = OutputId(1);
/// A second output.
pub const SIDE: OutputId = OutputId(2);

/// The window with id `n`.
pub fn w(n: u64) -> WindowId {
    WindowId(n)
}

/// A rectangle.
pub fn rect(x: f64, y: f64, w: f64, h: f64) -> Rect {
    Rect::new(x, y, w, h)
}

/// An output's configuration.
pub fn output(x: f64, width: u32, height: u32, scale: f64) -> OutputConfig {
    OutputConfig {
        position: Point { x, y: 0.0 },
        physical_width: width,
        physical_height: height,
        scale,
    }
}

pub struct Model {
    pub layout: Layout,
}

impl Model {
    /// The owner's seat with one output, `MAIN`, 1920 × 1080 at scale 1, gaps of 16.
    pub fn new() -> Model {
        Model::with(Options::default())
    }

    /// The same, with these options.
    pub fn with(options: Options) -> Model {
        let mut model = Model::bare(options);
        model.ok(|l| l.add_seat(OWNER));
        model.ok(|l| l.add_output(OWNER, MAIN, output(0.0, 1920, 1080, 1.0)));
        model
    }

    /// No seat, no output.
    pub fn bare(options: Options) -> Model {
        Model {
            layout: Layout::new(options).expect("valid options"),
        }
    }

    /// Runs an operation that must succeed, then checks every invariant.
    #[track_caller]
    pub fn ok(&mut self, op: impl FnOnce(&mut Layout) -> Result<(), LayoutError>) {
        if let Err(error) = op(&mut self.layout) {
            panic!("the operation failed: {error}");
        }
        self.check();
    }

    /// Runs an operation that must fail with `expected`, then checks that the model did not
    /// change.
    #[track_caller]
    pub fn refused(
        &mut self,
        expected: LayoutError,
        op: impl FnOnce(&mut Layout) -> Result<(), LayoutError>,
    ) {
        let before = format!("{:?}", self.layout);
        assert_eq!(op(&mut self.layout), Err(expected));
        assert_eq!(
            before,
            format!("{:?}", self.layout),
            "a refused operation changed the model"
        );
        self.check();
    }

    #[track_caller]
    pub fn check(&self) {
        if let Err(broken) = self.layout.check_invariants() {
            panic!("invariant broken: {broken}");
        }
    }

    /// Opens window `n` on the owner's seat, tiled.
    #[track_caller]
    pub fn open(&mut self, n: u64) {
        self.open_with(OWNER, n, OpenOptions::default());
    }

    /// Opens window `n` on `seat` with `options`.
    #[track_caller]
    pub fn open_with(&mut self, seat: SeatId, n: u64, options: OpenOptions) {
        self.ok(|l| l.open_window(seat, w(n), options));
    }

    /// What `output` shows, in drawing order.
    pub fn placed(&self, output: OutputId) -> Vec<Placed> {
        self.layout.arrange(output).unwrap()
    }

    /// The windows `output` shows, in drawing order.
    pub fn shown(&self, output: OutputId) -> Vec<u64> {
        self.placed(output).iter().map(|p| p.window.0).collect()
    }

    /// Window `n`'s rectangle on the output that shows it.
    #[track_caller]
    pub fn rect(&self, n: u64) -> Rect {
        self.placement(n).rect
    }

    /// Window `n` as an output shows it.
    #[track_caller]
    pub fn placement(&self, n: u64) -> Placed {
        let output = self
            .info(n)
            .output
            .unwrap_or_else(|| panic!("window {n} is on no output"));
        self.placed(output)
            .into_iter()
            .find(|p| p.window == w(n))
            .unwrap_or_else(|| panic!("window {n} is not shown"))
    }

    /// Where window `n` is.
    #[track_caller]
    pub fn info(&self, n: u64) -> WindowInfo {
        self.layout
            .window(w(n))
            .unwrap_or_else(|| panic!("no window {n}"))
    }

    /// The owner's focused window.
    pub fn focused(&self) -> Option<u64> {
        self.layout.focused(OWNER).map(|w| w.0)
    }

    /// The names of `output`'s workspaces, `-` for an unnamed one, and the index shown.
    pub fn workspaces(&self, output: OutputId) -> (Vec<String>, usize) {
        let list = self.layout.workspaces(output).unwrap();
        let shown = list.iter().position(|w| w.shown).unwrap();
        let names = list
            .into_iter()
            .map(|w| w.name.unwrap_or_else(|| "-".to_owned()))
            .collect();
        (names, shown)
    }
}

/// Asserts two rectangles are equal to a millionth of a unit.
#[track_caller]
pub fn assert_rect(actual: Rect, expected: Rect) {
    let close = |a: f64, b: f64| (a - b).abs() < 1e-6;
    assert!(
        close(actual.x, expected.x)
            && close(actual.y, expected.y)
            && close(actual.w, expected.w)
            && close(actual.h, expected.h),
        "{actual:?} is not {expected:?}"
    );
}
