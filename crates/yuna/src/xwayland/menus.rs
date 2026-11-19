//! Override-redirect windows on Yuna's side: menus, tooltips, a combo box's list, a drag's icon,
//! the windows an X11 client places and maps itself, which no manager is asked about
//! ([ADR-0066](../../../../docs/adr/0066-yunas-own-x11-window-manager-on-x11rb.md)). They are
//! shown once mapped and paired with a buffer, drawn through the X11 element as any X11 surface
//! is, never in the layout and never focused by Yuna: X11 menus grab the input within Xwayland.
//!
//! A menu shown, moved, or naming another window in `WM_TRANSIENT_FOR` is given a window of the
//! layout by the first of `State::x11_menu_place`'s rules that finds one: the window it names,
//! else its own client's window shown under its corner, else its client's topmost. A window that
//! leaves the layout gives its menus to the rules again. A menu with no window is drawn alone, at
//! its X11 position, over every window of the outputs of its seat it lies on.
//!
//! A menu drawn with a window keeps the offset from that window's X11 corner it had when it was
//! placed, so that it slides with the window's drawn rectangle as xdg popups do
//! ([ADR-0118](../../../../docs/adr/0118-frames-presentation-times-and-the-clock.md)):
//! the frame loop draws it just above the window, at the window's surface's corner plus that
//! offset × s / S, exactly the offset at s = S, cut to the output, and the pointer is measured
//! from that same corner (`input.rs`). Its surface is one of its window's surface trees, called
//! back and told presented with them (`frames.rs`). A menu over no window has its own frame
//! callbacks: from each output that draws it, else at the hidden windows' rate.
//!
//! An X11 client shows [`MAX_MENUS_PER_CLIENT`] menus at once, and one Xwayland
//! [`MAX_MENUS_PER_XWAYLAND`], all its clients together; each constant says what a menu past it
//! waits for. The frame loop and the pointer walk only the menus shown, never every
//! override-redirect window, which a client may have by the thousand.

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use smithay::reexports::wayland_server::Resource;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{Physical, Point};
use x11rb::protocol::xproto::Window;
use yuna_layout::{OutputId, WindowId};

use super::properties::X11Rect;
use super::windows::{X11Server, X11Windows, x11_origin};
use crate::output::VirtualOutput;
use crate::render;
use crate::shell::has_buffer;
use crate::state::State;

/// The most override-redirect windows one X11 client may have shown at once, as a window may
/// have 64 popups ([ADR-0131](../../../../docs/adr/0131-bounds-on-what-a-client-or-an-agent-may-ask.md)):
/// a menu and its submenus, a tooltip and a list are a handful. The frame loop and the pointer
/// walk the shown ones at each refresh and event. One past it is not shown until it is mapped or
/// commits again, logged once per X11 client: only the client past its cap waits, and a client
/// that floods with thousands of menus, then leaves, leaves at once, none of them shown in turn.
pub const MAX_MENUS_PER_CLIENT: usize = 64;

/// The most override-redirect windows one Xwayland may have shown at once, all its X11 clients
/// together. An X11 client is a connection, and one program may open many, each with its own
/// [`MAX_MENUS_PER_CLIENT`]: up to Xwayland's limit of clients, thousands of menus that the frame
/// loop and the pointer would walk at each refresh and event. Four clients at their own cap, far
/// more than the handful a seat's programs show at once. One past it waits, logged once per
/// Xwayland. When a menu of the same Xwayland goes, those waiting show, in X11's stacking order,
/// lowest in X11's stacking first, as far as the two bounds let them: a program that floods its
/// Xwayland with menus over several connections delays another's tooltip until its own go,
/// never for good. Nothing is kept for them: they are found among the mapped override-redirect
/// windows, which every menu shown or hidden walks already.
pub const MAX_MENUS_PER_XWAYLAND: usize = 256;

/// Where an override-redirect window shown is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MenuPlace {
    /// With the managed window `parent`, `offset` X11 pixels from that window's X11 corner as
    /// Yuna had told it when the menu was placed.
    With { parent: Window, offset: (i32, i32) },
    /// At its own X11 position, over every window of the outputs it lies on.
    Alone,
}

/// The override-redirect windows one output shows, as its frame draws them and its hit tests
/// find them.
#[derive(Debug, Default)]
pub(crate) struct ShownMenus {
    /// Those over no window that show on the output, topmost first.
    pub lone: Vec<LoneMenu>,
    /// Each window's, topmost first.
    with: HashMap<WindowId, Vec<AnchoredMenu>>,
}

impl ShownMenus {
    /// The menus drawn with `window`, topmost first.
    pub(crate) fn with(&self, window: WindowId) -> &[AnchoredMenu] {
        self.with.get(&window).map_or(&[], Vec::as_slice)
    }
}

/// A menu over no window, on one output.
#[derive(Debug)]
pub(crate) struct LoneMenu {
    /// The Xwayland it is a window of.
    pub server: u64,
    pub window: Window,
    pub surface: WlSurface,
    /// Its surface's top-left corner, in the output's physical pixels.
    pub corner: Point<i32, Physical>,
    /// XWayland's scale, S.
    pub x11_scale: f64,
}

/// A menu drawn with a window.
#[derive(Debug)]
pub(crate) struct AnchoredMenu {
    pub window: Window,
    pub surface: WlSurface,
    /// Its offset from its window's surface's corner, in X11 pixels.
    pub offset: (i32, i32),
}

/// Where an output's last frame drew an override-redirect window, as
/// [`Compositor::drawn_x11_menus`](crate::Compositor::drawn_x11_menus) gives it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DrawnX11Menu {
    /// Its X11 window.
    pub window: u32,
    /// The window it was drawn with; none for one drawn at its own X11 position.
    pub parent: Option<WindowId>,
    /// Its whole surface's rectangle in the output's physical pixels, before it was cut to the
    /// output: its left edge, its top edge, its width, its height.
    pub rect: (i32, i32, i32, i32),
}

impl X11Server {
    /// Whether a new menu of `client` waits, its client or this Xwayland showing their most
    /// already: said once per X11 client, and once per Xwayland. One that this Xwayland's bound
    /// refuses shows once another goes ([`State::x11_show_waiting_menus`]); one past its own
    /// client's cap, at its next map or commit.
    fn refuses_menu_of(&mut self, client: u32) -> bool {
        let of_client = self.menus_shown_of.get(&client).copied().unwrap_or(0);
        if of_client >= MAX_MENUS_PER_CLIENT {
            if self.menu_refusals_said.insert(client) {
                tracing::warn!(
                    cap = MAX_MENUS_PER_CLIENT,
                    "an X11 menu is not shown: its client has its most; said once per client"
                );
            }
            return true;
        }
        if self.shown_menus.len() >= MAX_MENUS_PER_XWAYLAND {
            if !std::mem::replace(&mut self.menu_bound_said, true) {
                tracing::warn!(
                    bound = MAX_MENUS_PER_XWAYLAND,
                    "an X11 menu is not shown: its Xwayland shows its most; said once per Xwayland"
                );
            }
            self.menus_waiting = true;
            return true;
        }
        false
    }
}

impl X11Windows {
    /// Checks, for tests, that what the hot paths read of each Xwayland's menus agrees with its
    /// windows: the shown menus, in X11's stacking order, within [`MAX_MENUS_PER_XWAYLAND`];
    /// their count per client; the menus drawn with each window. Says which rule is broken.
    pub(crate) fn check_menus(&self) -> Result<(), String> {
        for record in self.servers.values() {
            let menu_of = |window: &Window| record.windows.get(window).and_then(|w| w.menu);
            let shown: Vec<Window> = record
                .menus
                .iter()
                .copied()
                .filter(|menu| menu_of(menu).is_some())
                .collect();
            if shown != record.shown_menus {
                return Err(format!(
                    "the shown menus are {:?}, not {shown:?}",
                    record.shown_menus
                ));
            }
            let all_shown = record
                .windows
                .values()
                .filter(|x11_window| x11_window.menu.is_some())
                .count();
            if all_shown != shown.len() {
                return Err(format!(
                    "{all_shown} menus are shown, {} of them mapped",
                    shown.len()
                ));
            }
            if shown.len() > MAX_MENUS_PER_XWAYLAND {
                return Err(format!(
                    "{} menus are shown, past the bound of {MAX_MENUS_PER_XWAYLAND}",
                    shown.len()
                ));
            }
            let mut counts: HashMap<u32, usize> = HashMap::new();
            let mut with: HashMap<Window, Vec<Window>> = HashMap::new();
            for menu in &shown {
                let Some(x11_window) = record.windows.get(menu) else {
                    continue;
                };
                *counts.entry(x11_window.client).or_default() += 1;
                if let Some(MenuPlace::With { parent, .. }) = x11_window.menu {
                    with.entry(parent).or_default().push(*menu);
                }
            }
            if counts != record.menus_shown_of {
                return Err(format!(
                    "the menus counted per client, {:?}, are not those shown, {counts:?}",
                    record.menus_shown_of
                ));
            }
            let sorted = |index: &HashMap<Window, Vec<Window>>| {
                let mut index: Vec<(Window, Vec<Window>)> = index
                    .iter()
                    .map(|(&parent, menus)| {
                        let mut menus = menus.clone();
                        menus.sort_unstable();
                        (parent, menus)
                    })
                    .collect();
                index.sort_unstable();
                index
            };
            if sorted(&with) != sorted(&record.menus_with) {
                return Err(format!(
                    "the menus drawn with each window are {:?}, not {with:?}",
                    record.menus_with
                ));
            }
        }
        Ok(())
    }
}

/// Whether `rect` holds the point (`x`, `y`).
fn holds(rect: X11Rect, (x, y): (i32, i32)) -> bool {
    let (x, y) = (i64::from(x), i64::from(y));
    let (left, top) = (i64::from(rect.x), i64::from(rect.y));
    x >= left && y >= top && x < left + i64::from(rect.width) && y < top + i64::from(rect.height)
}

impl State {
    /// Shows or hides `window` as an override-redirect window, as it is now: shown while it is
    /// mapped, override-redirect, and paired with a surface that has a buffer, unless its client
    /// has [`MAX_MENUS_PER_CLIENT`] shown already, or its Xwayland [`MAX_MENUS_PER_XWAYLAND`].
    /// Shown, it is placed (`x11_menu_place`); a change is drawn at its seat's outputs' next
    /// refresh.
    pub(super) fn x11_update_menu(&mut self, server: u64, window: Window) {
        let Some(record) = self.x11.servers.get(&server) else {
            return;
        };
        let Some(x11_window) = record.windows.get(&window) else {
            return;
        };
        let showable = x11_window.mapped
            && x11_window.override_redirect
            && record
                .pairing
                .surface(window)
                .is_some_and(|surface| surface.is_alive() && has_buffer(surface));
        let client = x11_window.client;
        match (showable, x11_window.menu.is_some()) {
            (true, false) => {
                if self
                    .x11
                    .servers
                    .get_mut(&server)
                    .is_some_and(|record| record.refuses_menu_of(client))
                {
                    return;
                }
                let place = self.x11_menu_place(server, window);
                if let Some(x11_window) = self.x11_window_mut(server, window) {
                    x11_window.menu_frame_sent = None;
                }
                self.x11_set_menu(server, window, Some(place));
                self.x11_redraw(server);
            }
            (false, true) => {
                self.x11_set_menu(server, window, None);
                self.x11_redraw(server);
                self.x11_show_waiting_menus(server);
            }
            _ => {}
        }
    }

    /// A menu of Xwayland `server` went: those that wait for room, refused by the Xwayland's
    /// bound since the last walk found none, show, bottom to top in X11's stacking, the oldest
    /// mapped first, until the Xwayland shows its most again; one whose client shows its own most
    /// is refused again, and waits for its next map or commit. Without one waiting, nothing is
    /// walked.
    pub(super) fn x11_show_waiting_menus(&mut self, server: u64) {
        let Some(record) = self.x11.servers.get_mut(&server) else {
            return;
        };
        if !std::mem::take(&mut record.menus_waiting) {
            return;
        }
        let windows = &record.windows;
        let not_shown: Vec<Window> = record
            .menus
            .iter()
            .copied()
            .filter(|menu| {
                windows
                    .get(menu)
                    .is_some_and(|x11_window| x11_window.menu.is_none())
            })
            .collect();
        for menu in not_shown {
            let Some(record) = self.x11.servers.get_mut(&server) else {
                return;
            };
            if record.shown_menus.len() >= MAX_MENUS_PER_XWAYLAND {
                // Those left may wait still: the next menu that goes walks them.
                record.menus_waiting = true;
                return;
            }
            self.x11_update_menu(server, menu);
        }
    }

    /// Sets where override-redirect `window` is drawn, or that it is not shown, and keeps what
    /// the hot paths read in step: the shown menus in stacking order, their count per client,
    /// and the menus drawn with each window. The frame loop, the pointer and every window's
    /// surface trees walk those alone, never the mapped override-redirect windows, which a
    /// client may have by the thousand.
    pub(super) fn x11_set_menu(&mut self, server: u64, window: Window, place: Option<MenuPlace>) {
        let Some(record) = self.x11.servers.get_mut(&server) else {
            return;
        };
        let Some(x11_window) = record.windows.get_mut(&window) else {
            return;
        };
        let before = std::mem::replace(&mut x11_window.menu, place);
        let client = x11_window.client;
        if let Some(MenuPlace::With { parent, .. }) = before
            && let Some(menus) = record.menus_with.get_mut(&parent)
        {
            menus.retain(|&menu| menu != window);
            if menus.is_empty() {
                record.menus_with.remove(&parent);
            }
        }
        if let Some(MenuPlace::With { parent, .. }) = place {
            record.menus_with.entry(parent).or_default().push(window);
        }
        match (before.is_some(), place.is_some()) {
            (false, true) => *record.menus_shown_of.entry(client).or_default() += 1,
            (true, false) => {
                if let Some(count) = record.menus_shown_of.get_mut(&client) {
                    *count = count.saturating_sub(1);
                    if *count == 0 {
                        record.menus_shown_of.remove(&client);
                    }
                }
            }
            _ => {}
        }
        if before.is_some() != place.is_some() {
            record.order_shown_menus();
        }
    }

    /// An override-redirect window moved or changed size: shown, it is placed again from where
    /// its client put it.
    pub(super) fn x11_menu_configured(&mut self, server: u64, window: Window, rect: X11Rect) {
        let Some(x11_window) = self.x11_window_mut(server, window) else {
            return;
        };
        x11_window.geometry = rect;
        self.x11_place_menu_again(server, window);
    }

    /// A mapped override-redirect window's place in X11's stacking: just above `above` among
    /// the others, or at their bottom.
    pub(super) fn x11_menu_restacked(
        &mut self,
        server: u64,
        window: Window,
        above: Option<Window>,
    ) {
        let Some(record) = self.x11.servers.get_mut(&server) else {
            return;
        };
        if !record.windows.contains_key(&window) {
            return;
        }
        record.menus.retain(|&menu| menu != window);
        let at = above
            .and_then(|above| record.menus.iter().position(|&menu| menu == above))
            .map_or(0, |below| below + 1);
        record.menus.insert(at, window);
        let shown = record
            .windows
            .get(&window)
            .is_some_and(|x11_window| x11_window.menu.is_some());
        if shown {
            record.order_shown_menus();
            self.x11_redraw(server);
        }
    }

    /// Places `window` again if it is shown: its client moved it, or changed the window it names.
    pub(super) fn x11_place_menu_again(&mut self, server: u64, window: Window) {
        let shown = self
            .x11
            .servers
            .get(&server)
            .and_then(|record| record.windows.get(&window))
            .is_some_and(|x11_window| x11_window.menu.is_some());
        if !shown {
            return;
        }
        let place = self.x11_menu_place(server, window);
        self.x11_set_menu(server, window, Some(place));
        self.x11_redraw(server);
    }

    /// Places again the menus drawn with `parent`, which left the layout or went.
    pub(super) fn x11_place_menus_of(&mut self, server: u64, parent: Window) {
        let anchored = self
            .x11
            .servers
            .get(&server)
            .and_then(|record| record.menus_with.get(&parent))
            .cloned()
            .unwrap_or_default();
        for menu in anchored {
            self.x11_place_menu_again(server, menu);
        }
    }

    /// Arms the next refresh of every output of Xwayland `server`'s seat: a menu came, went or
    /// moved.
    pub(super) fn x11_redraw(&mut self, server: u64) {
        let Some(seat) = self.x11.servers.get(&server).map(|record| record.seat) else {
            return;
        };
        let outputs: Vec<OutputId> = self
            .outputs
            .iter()
            .filter(|output| output.seat == seat)
            .map(|output| output.id)
            .collect();
        for output in outputs {
            self.schedule_refresh_for_content(output, crate::rest::DrawCause::Popups);
        }
    }

    /// Where override-redirect `window` is drawn, by the first rule that gives it a window: the
    /// managed window its `WM_TRANSIENT_FOR` names, if it is open; else, among the managed
    /// windows of its own X11 client shown on an output, topmost first, the one shown on the
    /// output under its X11 corner whose X11 rectangle holds that corner; else the topmost of
    /// them. With none, it is drawn alone, at its X11 position.
    fn x11_menu_place(&self, server: u64, window: Window) -> MenuPlace {
        let Some(record) = self.x11.servers.get(&server) else {
            return MenuPlace::Alone;
        };
        let Some(menu) = record.windows.get(&window) else {
            return MenuPlace::Alone;
        };
        let corner = (menu.geometry.x, menu.geometry.y);
        let with = |parent: Window| -> Option<MenuPlace> {
            let told = record.windows.get(&parent)?;
            let at = told.told.unwrap_or(told.geometry);
            Some(MenuPlace::With {
                parent,
                offset: (corner.0.saturating_sub(at.x), corner.1.saturating_sub(at.y)),
            })
        };
        let named = menu
            .properties
            .transient_for
            .filter(|&parent| parent != window)
            .filter(|&parent| self.x11_layout_window(server, parent).is_some())
            .and_then(with);
        if let Some(place) = named {
            return place;
        }
        // Its client's windows open and shown, topmost first: by Yuna's last stacking, those
        // opened since on top.
        let height = |candidate: Window| {
            record
                .stacking
                .iter()
                .position(|&stacked| stacked == candidate)
                .unwrap_or(usize::MAX)
        };
        let mut shown: Vec<(Window, OutputId, X11Rect)> = record
            .windows
            .iter()
            .filter(|(_, candidate)| {
                candidate.client == menu.client && !candidate.override_redirect
            })
            .filter_map(|(&candidate, x11_candidate)| {
                let root = self.x11_root(server, candidate)?;
                let toplevel = self.toplevels.get(&root.id())?;
                toplevel.window?;
                let told = x11_candidate.told.unwrap_or(x11_candidate.geometry);
                Some((candidate, toplevel.shown_on?, told))
            })
            .collect();
        shown.sort_by_key(|&(candidate, _, _)| std::cmp::Reverse(height(candidate)));
        let logical = (
            f64::from(corner.0) / record.scale,
            f64::from(corner.1) / record.scale,
        );
        let under_corner = self
            .outputs
            .iter()
            .find(|output| {
                output.seat == record.seat
                    && logical.0 >= output.rect.x
                    && logical.0 < output.rect.right()
                    && logical.1 >= output.rect.y
                    && logical.1 < output.rect.bottom()
            })
            .map(|output| output.id);
        let holding = shown
            .iter()
            .find(|&&(_, output, told)| Some(output) == under_corner && holds(told, corner));
        holding
            .or(shown.first())
            .and_then(|&(parent, _, _)| with(parent))
            .unwrap_or(MenuPlace::Alone)
    }

    /// The override-redirect windows `output` shows, as its frame draws them: those drawn alone
    /// that show there, at their X11 position less the output's X11 corner × s / S; and each
    /// window's, by its id in the layout.
    pub(crate) fn x11_shown_menus(&self, output: &VirtualOutput) -> ShownMenus {
        let mut shown = ShownMenus::default();
        for (&server, record) in &self.x11.servers {
            if record.seat != output.seat {
                continue;
            }
            let origin = x11_origin(output, record.scale);
            for &window in record.shown_menus.iter().rev() {
                let Some(x11_window) = record.windows.get(&window) else {
                    continue;
                };
                let (Some(place), Some(surface)) =
                    (x11_window.menu, record.pairing.surface(window))
                else {
                    continue;
                };
                match place {
                    MenuPlace::Alone => {
                        let offset = (
                            x11_window.geometry.x.saturating_sub(origin.0),
                            x11_window.geometry.y.saturating_sub(origin.1),
                        );
                        let corner =
                            render::x11_offset_on(Point::default(), offset, record.scale, output);
                        if render::x11_menu_origin(surface, corner, record.scale, output).is_some()
                        {
                            shown.lone.push(LoneMenu {
                                server,
                                window,
                                surface: surface.clone(),
                                corner,
                                x11_scale: record.scale,
                            });
                        }
                    }
                    MenuPlace::With { parent, offset } => {
                        let Some(id) = self.x11_layout_window(server, parent) else {
                            continue;
                        };
                        shown.with.entry(id).or_default().push(AnchoredMenu {
                            window,
                            surface: surface.clone(),
                            offset,
                        });
                    }
                }
            }
        }
        shown
    }

    /// The surfaces of the menus drawn with the X11 window of `root`, topmost first: part of its
    /// surface trees (`frames.rs`).
    pub(crate) fn x11_menus_with(&self, root: &WlSurface) -> Vec<WlSurface> {
        let Some(x11) = self
            .toplevels
            .get(&root.id())
            .and_then(|toplevel| toplevel.x11())
        else {
            return Vec::new();
        };
        let Some(record) = self.x11.servers.get(&x11.server) else {
            return Vec::new();
        };
        // Most windows have none: the index answers without a walk.
        if !record.menus_with.contains_key(&x11.window) {
            return Vec::new();
        }
        record
            .shown_menus
            .iter()
            .rev()
            .filter(|&&menu| {
                record.windows.get(&menu).is_some_and(|x11_window| {
                    matches!(x11_window.menu, Some(MenuPlace::With { parent, .. })
                        if parent == x11.window)
                })
            })
            .filter_map(|&menu| record.pairing.surface(menu).cloned())
            .collect()
    }

    /// A commit of `surface`, an Xwayland surface that is no managed window's: an
    /// override-redirect window's may show or hide it, and arms what its window's commits arm,
    /// or, drawn alone, the refresh of each output that shows it; on none, its content update is
    /// discarded and its frame callbacks wait for the hidden windows' deadline.
    pub(crate) fn x11_menu_committed(&mut self, surface: &WlSurface) {
        let Some((server, window)) = self.x11_window_of_surface(surface) else {
            return;
        };
        self.x11_update_menu(server, window);
        let Some(place) = self
            .x11
            .servers
            .get(&server)
            .and_then(|record| record.windows.get(&window))
            .and_then(|x11_window| x11_window.menu)
        else {
            return;
        };
        match place {
            MenuPlace::With { parent, .. } => {
                if let Some(root) = self.x11_root(server, parent) {
                    self.schedule_for_commit(&root, surface);
                }
            }
            MenuPlace::Alone => {
                let outputs: Vec<OutputId> = self
                    .outputs
                    .iter()
                    .filter(|output| {
                        self.x11_shown_menus(output)
                            .lone
                            .iter()
                            .any(|lone| (lone.server, lone.window) == (server, window))
                    })
                    .map(|output| output.id)
                    .collect();
                if outputs.is_empty() {
                    let trees = [surface.clone()];
                    crate::frames::discard_presentation_feedback(&trees);
                    if crate::frames::waits_for_frame(&trees) {
                        let last = self
                            .x11
                            .servers
                            .get(&server)
                            .and_then(|record| record.windows.get(&window))
                            .and_then(|x11_window| x11_window.menu_frame_sent);
                        self.arm_hidden_frame(last);
                    }
                }
                let content = std::mem::take(&mut self.committed_content);
                let cause = crate::rest::DrawCause::Commit(self.committing_client);
                for output in outputs {
                    if content {
                        self.schedule_refresh_for_content(output, cause);
                    } else {
                        self.schedule_refresh(output, cause);
                    }
                }
            }
        }
    }

    /// The menus drawn alone that no output shows, with when each was last called back: they get
    /// their frame callbacks at the hidden windows' rate (`frames.rs`).
    pub(crate) fn x11_unseen_menus(&self) -> Vec<(WlSurface, Option<Duration>)> {
        let seen: HashSet<(u64, Window)> = self
            .outputs
            .iter()
            .flat_map(|output| self.x11_shown_menus(output).lone)
            .map(|lone| (lone.server, lone.window))
            .collect();
        let mut unseen = Vec::new();
        for (&server, record) in &self.x11.servers {
            for &window in &record.shown_menus {
                let Some(x11_window) = record.windows.get(&window) else {
                    continue;
                };
                let (Some(MenuPlace::Alone), Some(surface)) =
                    (x11_window.menu, record.pairing.surface(window))
                else {
                    continue;
                };
                if !seen.contains(&(server, window)) {
                    unseen.push((surface.clone(), x11_window.menu_frame_sent));
                }
            }
        }
        unseen
    }

    /// Records that the menu of `surface` was called back at `now`.
    pub(crate) fn x11_menu_called_back(&mut self, surface: &WlSurface, now: Duration) {
        if let Some((server, window)) = self.x11_window_of_surface(surface)
            && let Some(x11_window) = self.x11_window_mut(server, window)
        {
            x11_window.menu_frame_sent = Some(now);
        }
    }

    /// Where output `output`'s last frame drew each override-redirect window: from the windows it
    /// drew, and the menus shown now.
    pub(crate) fn describe_x11_menus(&self, output: OutputId) -> Vec<DrawnX11Menu> {
        let Some(output) = self.outputs.iter().find(|candidate| candidate.id == output) else {
            return Vec::new();
        };
        let menus = self.x11_shown_menus(output);
        let rect = |surface: &WlSurface, corner, x11_scale| {
            render::x11_menu_rect(surface, corner, x11_scale, output)
                .map(|rect| (rect.loc.x, rect.loc.y, rect.size.w, rect.size.h))
        };
        let mut drawn: Vec<DrawnX11Menu> = menus
            .lone
            .iter()
            .filter_map(|lone| {
                Some(DrawnX11Menu {
                    window: lone.window,
                    parent: None,
                    rect: rect(&lone.surface, lone.corner, lone.x11_scale)?,
                })
            })
            .collect();
        for window in output
            .drawn
            .iter()
            .filter(|window| window.rect.overlaps(&output.rect))
        {
            let Some(x11) = self
                .windows
                .get(&window.window)
                .and_then(|surface| self.toplevels.get(surface))
                .and_then(|toplevel| toplevel.x11())
            else {
                continue;
            };
            let corner = render::x11_surface_corner(
                self.x11.geometry_offset(x11),
                window.rect,
                x11.scale,
                output,
            );
            for menu in menus.with(window.window) {
                let at = render::x11_offset_on(corner, menu.offset, x11.scale, output);
                if let Some(rect) = rect(&menu.surface, at, x11.scale) {
                    drawn.push(DrawnX11Menu {
                        window: menu.window,
                        parent: Some(window.window),
                        rect,
                    });
                }
            }
        }
        drawn
    }
}
