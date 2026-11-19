//! Costs of the commit and arrangement paths that must follow the work asked, never grow faster
//! (Stage 1's audit, m1.13): one commit of a window applying its whole tree of subsurfaces, an
//! arrangement beside thousands of windows refused by the cap, and new windows' first commits on
//! a seat full of windows. Each story checks what it can count, never a time: a time depends on
//! the machine. Each also prints its times, and runs at the size `YUNA_COST_SIZE` gives, for the
//! measurements made in release (`docs/measurements.md`).
//!
//! A test [`Client`] makes one subsurface at a time; [`Trees`] makes thousands, on an event
//! queue of its own on the client's connection.

use std::os::fd::AsFd;
use std::time::{Duration, Instant};

use wayland_client::protocol::{
    wl_buffer, wl_callback, wl_compositor, wl_registry, wl_shm, wl_shm_pool, wl_subcompositor,
    wl_subsurface, wl_surface,
};
use wayland_client::{Connection, Dispatch, EventQueue, QueueHandle, delegate_noop};
use yuna::yuna_layout::geometry::Size;
use yuna::yuna_layout::{OpenOptions, WindowId};
use yuna::{Compositor, HIDDEN_FRAME_INTERVAL, MAX_WINDOWS_PER_CLIENT, OWNER_SEAT};

use crate::client::{Client, memory_file};
use crate::{OUTPUT, manual_compositor};

const GREY: u32 = 0xFF_80_80_80;

/// The size a story runs at: `default`, or `YUNA_COST_SIZE` when it is set, for a measurement.
fn size(default: usize) -> usize {
    std::env::var("YUNA_COST_SIZE")
        .ok()
        .and_then(|size| size.parse().ok())
        .unwrap_or(default)
}

/// The globals [`Trees`] binds on its own queue, and the frame callbacks its surfaces got.
#[derive(Default)]
struct Globals {
    compositor: Option<wl_compositor::WlCompositor>,
    subcompositor: Option<wl_subcompositor::WlSubcompositor>,
    shm: Option<wl_shm::WlShm>,
    frames_done: usize,
}

impl Dispatch<wl_registry::WlRegistry, ()> for Globals {
    fn event(
        globals: &mut Self,
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
            "wl_compositor" => {
                globals.compositor = Some(registry.bind(name, version.min(6), qh, ()))
            }
            "wl_subcompositor" => globals.subcompositor = Some(registry.bind(name, 1, qh, ())),
            "wl_shm" => globals.shm = Some(registry.bind(name, 1, qh, ())),
            _ => {}
        }
    }
}

impl Dispatch<wl_callback::WlCallback, ()> for Globals {
    fn event(
        globals: &mut Self,
        _: &wl_callback::WlCallback,
        event: wl_callback::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_callback::Event::Done { .. } = event {
            globals.frames_done += 1;
        }
    }
}

delegate_noop!(Globals: wl_compositor::WlCompositor);
delegate_noop!(Globals: wl_subcompositor::WlSubcompositor);
delegate_noop!(Globals: wl_subsurface::WlSubsurface);
delegate_noop!(Globals: wl_shm_pool::WlShmPool);
delegate_noop!(Globals: ignore wl_surface::WlSurface);
delegate_noop!(Globals: ignore wl_shm::WlShm);
delegate_noop!(Globals: ignore wl_buffer::WlBuffer);

/// One surface of a tree and its subsurface object.
struct Hung {
    surface: wl_surface::WlSurface,
    subsurface: wl_subsurface::WlSubsurface,
}

/// Trees of subsurfaces under a test client's windows, made on an event queue of its own on the
/// client's connection: the client's round trips send its requests and read its events.
struct Trees {
    queue: EventQueue<Globals>,
    globals: Globals,
}

impl Trees {
    fn bind(client: &mut Client, compositor: &mut Compositor) -> Trees {
        let queue = client.connection().new_event_queue();
        client
            .connection()
            .display()
            .get_registry(&queue.handle(), ());
        let mut trees = Trees {
            queue,
            globals: Globals::default(),
        };
        client.roundtrip(compositor);
        trees.dispatch();
        client.roundtrip(compositor);
        assert!(
            trees.globals.compositor.is_some()
                && trees.globals.subcompositor.is_some()
                && trees.globals.shm.is_some(),
            "a global is missing"
        );
        trees
    }

    /// Handles the events the client's last round trip read for this queue.
    fn dispatch(&mut self) {
        self.queue
            .dispatch_pending(&mut self.globals)
            .expect("the trees' events are handled");
    }

    fn surface(&self) -> wl_surface::WlSurface {
        let compositor = self.globals.compositor.as_ref().expect("wl_compositor");
        compositor.create_surface(&self.queue.handle(), ())
    }

    fn hang(&self, surface: &wl_surface::WlSurface, parent: &wl_surface::WlSurface) -> Hung {
        let subcompositor = self
            .globals
            .subcompositor
            .as_ref()
            .expect("wl_subcompositor");
        Hung {
            surface: surface.clone(),
            subsurface: subcompositor.get_subsurface(surface, parent, &self.queue.handle(), ()),
        }
    }

    /// A chain of `depth` synchronised subsurfaces under `parent`, each the parent of the next,
    /// top first, none committed. Made from the bottom up, so that hanging each costs the
    /// compositor one step rather than a walk of the chain above it; sent in batches, a round
    /// trip after each.
    fn chain(
        &mut self,
        client: &mut Client,
        compositor: &mut Compositor,
        parent: &wl_surface::WlSurface,
        depth: usize,
    ) -> Vec<Hung> {
        let surfaces: Vec<wl_surface::WlSurface> = (0..depth).map(|_| self.surface()).collect();
        let mut hung = Vec::with_capacity(depth);
        for (made, pair) in surfaces.windows(2).rev().enumerate() {
            hung.push(self.hang(&pair[1], &pair[0]));
            if made % 512 == 511 {
                client.roundtrip(compositor);
            }
        }
        if let Some(top) = surfaces.first() {
            hung.push(self.hang(top, parent));
        }
        client.roundtrip(compositor);
        self.dispatch();
        hung.reverse();
        hung
    }

    /// `count` synchronised subsurfaces directly under `parent`, none committed; sent in
    /// batches, a round trip after each.
    fn fan(
        &mut self,
        client: &mut Client,
        compositor: &mut Compositor,
        parent: &wl_surface::WlSurface,
        count: usize,
    ) -> Vec<Hung> {
        let mut hung = Vec::with_capacity(count);
        for made in 0..count {
            hung.push(self.hang(&self.surface(), parent));
            if made % 512 == 511 {
                client.roundtrip(compositor);
            }
        }
        client.roundtrip(compositor);
        self.dispatch();
        hung
    }

    /// A buffer of `side` × `side` grey pixels, for a surface to attach.
    fn buffer(&self, side: i32) -> wl_buffer::WlBuffer {
        let qh = self.queue.handle();
        let stride = side * 4;
        let bytes = u64::try_from(stride * side).expect("a small buffer");
        let file = memory_file(bytes);
        let shm = self.globals.shm.as_ref().expect("wl_shm");
        let pool = shm.create_pool(file.as_fd(), stride * side, &qh, ());
        let buffer = pool.create_buffer(0, side, side, stride, wl_shm::Format::Argb8888, &qh, ());
        pool.destroy();
        buffer
    }

    /// Asks a frame callback with `surface`'s next commit.
    fn frame(&self, surface: &wl_surface::WlSurface) {
        surface.frame(&self.queue.handle(), ());
    }

    /// Attaches a new buffer of `side` × `side` pixels to `surface` and commits it.
    fn show(&self, surface: &wl_surface::WlSurface, side: i32) {
        surface.attach(Some(&self.buffer(side)), 0, 0);
        surface.damage_buffer(0, 0, side, side);
        surface.commit();
    }
}

/// A client with one window of 100 × 100 pixels, mapped and drawn.
fn a_mapped_window() -> (Compositor, Client, usize) {
    let mut compositor = manual_compositor();
    let mut client = Client::connect(&mut compositor);
    let window = client.open_window(&mut compositor);
    client.ack(window);
    client.attach(window, 100, 100, GREY);
    client.commit(window);
    client.roundtrip(&mut compositor);
    settle(&mut compositor, &mut client);
    (compositor, client, window)
}

/// Lets the clock run long enough for every refresh and motion armed to end.
fn settle(compositor: &mut Compositor, client: &mut Client) {
    compositor
        .advance(Duration::from_millis(500))
        .expect("the manual clock moves");
    client.roundtrip(compositor);
}

/// The size of the only window on the output, as the layout draws it: the size its client's
/// window geometry gave.
fn drawn_size(compositor: &Compositor) -> (f64, f64) {
    let placed = compositor.layout().arrange(OUTPUT).expect("the output");
    assert_eq!(placed.len(), 1, "{placed:?}");
    (placed[0].rect.w, placed[0].rect.h)
}

/// The window commits once, applying the `added` synchronised subsurfaces hung under it: how
/// many times Yuna's commit handler ran for it, once for each surface the commit applied, and
/// the time of the commit and of the refresh after it, printed.
fn commit_the_tree(
    compositor: &mut Compositor,
    client: &mut Client,
    window: usize,
    added: usize,
    shape: &str,
) {
    let commits_before = compositor.counters().commits;
    let started = Instant::now();
    client.commit(window);
    client.roundtrip(compositor);
    let commit = started.elapsed();
    let started = Instant::now();
    compositor
        .advance(Duration::from_millis(10))
        .expect("the manual clock moves");
    let refresh = started.elapsed();
    eprintln!("{shape} of {added}: the window's commit took {commit:?}, the refresh {refresh:?}");
    assert_eq!(
        compositor.counters().commits - commits_before,
        added as u64 + 1
    );
    assert_eq!(compositor.windows().len(), 1);
}

/// One commit of a window applies the whole chain of synchronised subsurfaces hung under it:
/// Smithay calls the commit handler once for each of its surfaces, and Yuna reads the window's
/// tree once, not once a call (m1.13-audit-core-1).
#[test]
fn one_commit_of_a_window_applies_its_whole_chain() {
    let depth = size(256);
    let (mut compositor, mut client, window) = a_mapped_window();
    let mut trees = Trees::bind(&mut client, &mut compositor);
    let parent = client.window(window).surface.clone();
    let _chain = trees.chain(&mut client, &mut compositor, &parent, depth);
    commit_the_tree(&mut compositor, &mut client, window, depth, "a chain");
}

/// As [`one_commit_of_a_window_applies_its_whole_chain`], for subsurfaces hung side by side
/// under the window.
#[test]
fn one_commit_of_a_window_applies_its_whole_fan() {
    let count = size(512);
    let (mut compositor, mut client, window) = a_mapped_window();
    let mut trees = Trees::bind(&mut client, &mut compositor);
    let parent = client.window(window).surface.clone();
    let _fan = trees.fan(&mut client, &mut compositor, &parent, count);
    commit_the_tree(&mut compositor, &mut client, window, count, "a fan");
}

/// A window of 100 × 100 pixels hangs a chain of 190 synchronised subsurfaces, each 10 × 10, one
/// unit right of and below its parent: once the window commits, its geometry is the bounding box
/// of its tree, 200 × 200, though the handler ran for the deepest surface first. With a geometry
/// set from (4, 4), the window is that box from there. The deepest surface growing to 30 × 30
/// shows at the window's next commit alone.
#[test]
fn a_deep_chain_gives_its_window_the_bounding_box_of_its_tree() {
    let (mut compositor, mut client, window) = a_mapped_window();
    let mut trees = Trees::bind(&mut client, &mut compositor);
    let parent = client.window(window).surface.clone();
    let chain = trees.chain(&mut client, &mut compositor, &parent, 190);
    for hung in &chain {
        hung.subsurface.set_position(1, 1);
        trees.show(&hung.surface, 10);
    }
    client.roundtrip(&mut compositor);
    // Synchronised: nothing shows before the window commits.
    settle(&mut compositor, &mut client);
    assert_eq!(drawn_size(&compositor), (100.0, 100.0));

    client.commit(window);
    client.roundtrip(&mut compositor);
    settle(&mut compositor, &mut client);
    assert_eq!(drawn_size(&compositor), (200.0, 200.0));

    client.set_window_geometry(window, (4, 4), (300, 300));
    client.commit(window);
    client.roundtrip(&mut compositor);
    settle(&mut compositor, &mut client);
    assert_eq!(drawn_size(&compositor), (196.0, 196.0));

    let deepest = &chain.last().expect("a chain").surface;
    trees.show(deepest, 30);
    client.roundtrip(&mut compositor);
    settle(&mut compositor, &mut client);
    assert_eq!(drawn_size(&compositor), (196.0, 196.0));
    client.commit(window);
    client.roundtrip(&mut compositor);
    settle(&mut compositor, &mut client);
    assert_eq!(drawn_size(&compositor), (216.0, 216.0));
}

/// A desynchronised subsurface's own commit applies the synchronised chain under it, without
/// the window's: the window's geometry follows its tree all the same. The subsurface is 10 × 10
/// at the window's corner, and its chain of 140 goes one unit further at each step.
#[test]
fn a_desynchronised_subsurface_commits_the_chain_under_it_into_the_geometry() {
    let (mut compositor, mut client, window) = a_mapped_window();
    let mut trees = Trees::bind(&mut client, &mut compositor);
    let parent = client.window(window).surface.clone();
    let top = trees.hang(&trees.surface(), &parent);
    top.subsurface.set_desync();
    let chain = trees.chain(&mut client, &mut compositor, &top.surface, 140);
    for hung in &chain {
        hung.subsurface.set_position(1, 1);
        trees.show(&hung.surface, 10);
    }
    client.roundtrip(&mut compositor);
    settle(&mut compositor, &mut client);
    assert_eq!(drawn_size(&compositor), (100.0, 100.0));

    trees.show(&top.surface, 10);
    client.roundtrip(&mut compositor);
    settle(&mut compositor, &mut client);
    assert_eq!(drawn_size(&compositor), (150.0, 150.0));
}

/// A desynchronised subsurface that committed while its parent was still synchronised keeps its
/// place first in Smithay's next transaction of it: once the parent is desynchronised, its next
/// commit runs the commit handler for it before the synchronised child that commit applies. The
/// child asks a frame callback in that commit, on a window no output shows: it gets it at the
/// hidden windows' rate all the same, as the handler's call for the child, the commit's last,
/// arms it.
#[test]
fn a_child_applied_after_the_surface_committed_still_gets_its_frame_callback() {
    let mut compositor = manual_compositor();
    // Its id taken, the layout refuses the window: no output shows it.
    compositor
        .change_layout(|layout| layout.open_window(OWNER_SEAT, WindowId(1), OpenOptions::default()))
        .expect("the id is taken");
    let mut client = Client::connect(&mut compositor);
    let window = client.open_window(&mut compositor);
    client.ack(window);
    client.attach(window, 100, 100, GREY);
    client.commit(window);
    client.roundtrip(&mut compositor);
    let mut trees = Trees::bind(&mut client, &mut compositor);
    let root = client.window(window).surface.clone();
    let parent = trees.hang(&trees.surface(), &root);
    let middle = trees.hang(&trees.surface(), &parent.surface);
    middle.subsurface.set_desync();
    let child = trees.hang(&trees.surface(), &middle.surface);
    trees.show(&child.surface, 10);
    trees.show(&middle.surface, 10);
    trees.show(&parent.surface, 10);
    client.commit(window);
    client.roundtrip(&mut compositor);
    settle(&mut compositor, &mut client);
    assert_eq!(compositor.next_deadline(), None);

    // Committed while its parent is synchronised: its state waits for the parent's.
    trees.show(&middle.surface, 10);
    parent.subsurface.set_desync();
    trees.frame(&child.surface);
    child.surface.commit();
    trees.show(&middle.surface, 10);
    client.roundtrip(&mut compositor);
    compositor
        .advance(HIDDEN_FRAME_INTERVAL)
        .expect("the manual clock moves");
    client.roundtrip(&mut compositor);
    trees.dispatch();
    assert_eq!(trees.globals.frames_done, 1);
}

/// One client opens 1,000 windows, its cap, then `refused` more, which the cap refuses: they
/// get their frame callbacks at the hidden windows' rate. An arrangement then looks at the
/// windows in the layout alone, never at the refused ones, as Yuna counts them
/// (`Counters::windows_looked_at_for_hidden_frames`): its cost follows the windows the layout
/// holds, however many windows the cap refuses (m1.13-audit-core-5).
#[test]
fn an_arrangement_looks_at_the_windows_in_the_layout_alone() {
    let refused = size(200);
    let total = MAX_WINDOWS_PER_CLIENT + refused;
    let mut compositor = manual_compositor();
    let mut client = Client::connect(&mut compositor);
    let windows: Vec<usize> = (0..total)
        .map(|made| {
            let window = client.create_window();
            client.commit(window);
            if made % 500 == 499 {
                client.roundtrip(&mut compositor);
            }
            window
        })
        .collect();
    client.roundtrip(&mut compositor);
    for (made, &window) in windows.iter().enumerate() {
        client.ack(window);
        client.attach(window, 1, 1, GREY);
        client.request_frame(window);
        client.commit(window);
        if made % 500 == 499 {
            client.roundtrip(&mut compositor);
        }
    }
    client.roundtrip(&mut compositor);
    settle(&mut compositor, &mut client);
    assert_eq!(compositor.windows().len(), MAX_WINDOWS_PER_CLIENT);

    let mut times = Vec::new();
    for _ in 0..5 {
        let started = Instant::now();
        compositor.change_layout(|layout| {
            let _ = layout.focus(OWNER_SEAT, yuna::yuna_layout::Direction::Left);
        });
        compositor
            .advance(Duration::from_millis(20))
            .expect("the manual clock moves");
        times.push(started.elapsed());
        client.roundtrip(&mut compositor);
    }
    eprintln!(
        "{total} windows asked, {refused} refused: an arrangement and its frame took {times:?}"
    );

    // Every window had its frame callbacks: one arrangement alone looks at windows now.
    assert_eq!(compositor.next_deadline(), None);
    let before = compositor.counters().windows_looked_at_for_hidden_frames;
    compositor.change_layout(|layout| {
        let _ = layout.focus(OWNER_SEAT, yuna::yuna_layout::Direction::Left);
    });
    compositor
        .advance(Duration::from_millis(20))
        .expect("the manual clock moves");
    let looked_at = compositor.counters().windows_looked_at_for_hidden_frames - before;
    assert_eq!(looked_at, MAX_WINDOWS_PER_CLIENT as u64);
}

/// `seated` windows of 1 × 1 pixel on the owner's seat, 1,000 a client, then one more client
/// whose 200 new windows make their initial commits at once: each is foretold the size it will
/// open at, a new column's, from the layout without a copy of the seat (m1.13-audit-core-6).
#[test]
fn initial_commits_on_a_full_seat_are_each_foretold_their_size() {
    let seated = size(MAX_WINDOWS_PER_CLIENT);
    let mut compositor = manual_compositor();
    let mut seat = Vec::new();
    for _ in 0..seated.div_ceil(MAX_WINDOWS_PER_CLIENT) {
        let mut client = Client::connect(&mut compositor);
        let windows: Vec<usize> = (0..MAX_WINDOWS_PER_CLIENT)
            .map(|_| {
                let window = client.create_window();
                client.commit(window);
                window
            })
            .collect();
        client.roundtrip(&mut compositor);
        for &window in &windows {
            client.ack(window);
            client.attach(window, 1, 1, GREY);
            client.commit(window);
        }
        client.roundtrip(&mut compositor);
        seat.push(client);
    }
    settle(&mut compositor, &mut seat[0]);
    let in_layout = compositor.windows().len();

    let mut opening = Client::connect(&mut compositor);
    let opened: Vec<usize> = (0..200).map(|_| opening.create_window()).collect();
    opening.roundtrip(&mut compositor);
    for &window in &opened {
        opening.commit(window);
    }
    let started = Instant::now();
    opening.roundtrip(&mut compositor);
    let took = started.elapsed();
    eprintln!(
        "{in_layout} windows on the seat: 200 initial commits took {took:?}, {:?} each",
        took / 200
    );
    let foretold = compositor
        .layout()
        .size_on_open(OWNER_SEAT, &Default::default())
        .expect("the owner's seat");
    assert_eq!(
        foretold,
        Size {
            w: 1256.0,
            h: 1408.0
        }
    );
    for &window in &opened {
        let configures = &opening.window(window).configures;
        assert_eq!(configures.len(), 1);
        assert_eq!((configures[0].width, configures[0].height), (1256, 1408));
    }
}
