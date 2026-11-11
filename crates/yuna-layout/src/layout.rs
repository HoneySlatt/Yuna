//! The whole model: seats, their outputs and workspaces, and the operations on them.

use std::collections::HashMap;

use crate::area::Area;
use crate::geometry::{Rect, Scale, Size};
use crate::scrolling::ColumnWidth;
use crate::workspace::{Arranged, TilingLayout, Workspace};
use crate::{
    Direction, LayoutError, OpenOptions, Options, OutputConfig, OutputId, Placed, SeatId, Tiling,
    WindowId, WindowInfo, WorkspaceInfo,
};

/// The layout of every seat's windows. See the crate's documentation.
#[derive(Debug, Clone)]
pub struct Layout {
    options: Options,
    seats: Vec<Seat>,
    /// What the model knows of each window beyond its place.
    windows: HashMap<WindowId, WindowRecord>,
}

/// A window's state that does not depend on where it is.
#[derive(Debug, Clone)]
struct WindowRecord {
    seat: SeatId,
    fake_fullscreen: bool,
    /// Where it last floated, relative to the working area, to float there again.
    floating_rect: Option<Rect>,
    /// It floated before going fullscreen, and floats again when it leaves fullscreen.
    float_after_fullscreen: bool,
}

#[derive(Debug, Clone)]
struct Seat {
    id: SeatId,
    outputs: Vec<Output>,
    /// The index in `outputs` of the output the seat works on; 0 when it has none.
    active: usize,
    /// Workspaces whose outputs are all gone, kept with their windows until an output comes.
    detached: Vec<Workspace>,
    /// Special workspaces not shown.
    hidden_specials: Vec<Workspace>,
}

#[derive(Debug, Clone)]
struct Output {
    id: OutputId,
    rect: Rect,
    scale: Scale,
    /// Never empty after an operation: the model gives an output without one an unnamed one.
    workspaces: Vec<Workspace>,
    /// The index in `workspaces` of the one shown.
    shown: usize,
    /// The special workspace shown over it.
    special: Option<Workspace>,
}

impl Output {
    fn area(&self, gap: f64) -> Area {
        Area::new(self.rect, self.scale, gap)
    }
}

/// Where a workspace is in its seat. Valid until the next change of the seat's tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Slot {
    Output { output: usize, index: usize },
    ShownSpecial { output: usize },
    Detached(usize),
    HiddenSpecial(usize),
}

/// What moves with a window from one workspace to another.
struct Moving {
    window: WindowId,
    floating: Option<Rect>,
    fullscreen: bool,
    /// Its column's width and full width, kept as niri keeps them.
    column: Option<(ColumnWidth, bool)>,
}

impl Layout {
    /// An empty model: no seat, no output, no window. Refused if an option is not finite or out
    /// of its range: a gap below 0 or above 1,000; a proportion below
    /// [`ColumnWidth::MIN_PROPORTION`] or above 1, or a fixed width above 100,000; an empty
    /// floating size, or one above 100,000.
    pub fn new(options: Options) -> Result<Layout, LayoutError> {
        let valid = options.gap.is_finite()
            && (0.0..=MAX_GAP).contains(&options.gap)
            && valid_width(options.default_width)
            && options.preset_widths.iter().all(|&w| valid_width(w))
            && valid_size(options.default_floating_size);
        if !valid {
            return Err(LayoutError::InvalidValue);
        }
        Ok(Layout {
            options,
            seats: Vec::new(),
            windows: HashMap::new(),
        })
    }

    // ----- Lookups -----

    fn seat_index(&self, seat: SeatId) -> Result<usize, LayoutError> {
        self.seats
            .iter()
            .position(|s| s.id == seat)
            .ok_or(LayoutError::UnknownSeat(seat))
    }

    /// The seat and output index of `output`.
    fn output_index(&self, output: OutputId) -> Result<(usize, usize), LayoutError> {
        self.seats
            .iter()
            .enumerate()
            .find_map(|(si, s)| {
                s.outputs
                    .iter()
                    .position(|o| o.id == output)
                    .map(|oi| (si, oi))
            })
            .ok_or(LayoutError::UnknownOutput(output))
    }

    /// The seat index and slot of the workspace holding `window`.
    fn find_window(&self, window: WindowId) -> Result<(usize, Slot), LayoutError> {
        let record = self
            .windows
            .get(&window)
            .ok_or(LayoutError::UnknownWindow(window))?;
        let si = self.seat_index(record.seat)?;
        let slot = self
            .slots(si)
            .into_iter()
            .find(|&slot| self.workspace(si, slot).contains(window))
            .ok_or(LayoutError::UnknownWindow(window))?;
        Ok((si, slot))
    }

    /// Every workspace slot of a seat.
    fn slots(&self, si: usize) -> Vec<Slot> {
        let seat = &self.seats[si];
        let mut slots = Vec::new();
        for (oi, output) in seat.outputs.iter().enumerate() {
            slots.extend(
                (0..output.workspaces.len()).map(|index| Slot::Output { output: oi, index }),
            );
            if output.special.is_some() {
                slots.push(Slot::ShownSpecial { output: oi });
            }
        }
        slots.extend((0..seat.detached.len()).map(Slot::Detached));
        slots.extend((0..seat.hidden_specials.len()).map(Slot::HiddenSpecial));
        slots
    }

    fn workspace(&self, si: usize, slot: Slot) -> &Workspace {
        let seat = &self.seats[si];
        match slot {
            Slot::Output { output, index } => &seat.outputs[output].workspaces[index],
            Slot::ShownSpecial { output } => seat.outputs[output].special.as_ref().expect(
                "a ShownSpecial slot is made only for an output showing a special workspace",
            ),
            Slot::Detached(i) => &seat.detached[i],
            Slot::HiddenSpecial(i) => &seat.hidden_specials[i],
        }
    }

    /// The workspace in `slot` and its area, when it has an output.
    fn workspace_mut(&mut self, si: usize, slot: Slot) -> (&mut Workspace, Option<Area>) {
        let gap = self.options.gap;
        let seat = &mut self.seats[si];
        match slot {
            Slot::Output { output, index } => {
                let output = &mut seat.outputs[output];
                let area = output.area(gap);
                (&mut output.workspaces[index], Some(area))
            }
            Slot::ShownSpecial { output } => {
                let output = &mut seat.outputs[output];
                let area = output.area(gap);
                let special = output.special.as_mut().expect(
                    "a ShownSpecial slot is made only for an output showing a special workspace",
                );
                (special, Some(area))
            }
            Slot::Detached(i) => (&mut seat.detached[i], None),
            Slot::HiddenSpecial(i) => (&mut seat.hidden_specials[i], None),
        }
    }

    /// The workspace a seat works in: the special one shown on its active output, else the
    /// workspace that output shows. None for a seat without an output.
    fn focused_slot(&self, si: usize) -> Option<Slot> {
        let seat = &self.seats[si];
        let output = seat.outputs.get(seat.active)?;
        Some(if output.special.is_some() {
            Slot::ShownSpecial {
                output: seat.active,
            }
        } else {
            Slot::Output {
                output: seat.active,
                index: output.shown,
            }
        })
    }

    /// The slot of the regular workspace named `name` in a seat.
    fn named_slot(&self, si: usize, name: &str) -> Option<Slot> {
        let seat = &self.seats[si];
        for (oi, output) in seat.outputs.iter().enumerate() {
            if let Some(index) = output
                .workspaces
                .iter()
                .position(|w| w.name.as_deref() == Some(name))
            {
                return Some(Slot::Output { output: oi, index });
            }
        }
        seat.detached
            .iter()
            .position(|w| w.name.as_deref() == Some(name))
            .map(Slot::Detached)
    }

    /// The slot of the regular workspace named `name`, created at the end of the seat's active
    /// output if it does not exist, or among the detached ones if the seat has no output.
    fn named_slot_or_create(&mut self, si: usize, name: &str) -> Slot {
        if let Some(slot) = self.named_slot(si, name) {
            return slot;
        }
        let tiling = self.options.default_tiling;
        let seat = &mut self.seats[si];
        match seat.outputs.get_mut(seat.active) {
            Some(output) => {
                let workspace = Workspace::new(Some(name.to_owned()), Some(output.id), tiling);
                output.workspaces.push(workspace);
                Slot::Output {
                    output: seat.active,
                    index: output.workspaces.len() - 1,
                }
            }
            None => {
                seat.detached
                    .push(Workspace::new(Some(name.to_owned()), None, tiling));
                Slot::Detached(seat.detached.len() - 1)
            }
        }
    }

    // ----- Keeping the tree tidy -----

    /// Shows workspace `index` on output `oi` of seat `si`; `tidy` then brings the output's pinned
    /// windows over to it. A special workspace shown over the output stays, as in Hyprland
    /// (`binds:hide_special_on_workspace_change` false).
    fn show(&mut self, si: usize, oi: usize, index: usize) {
        self.seats[si].outputs[oi].shown = index;
    }

    /// Hides the special workspace shown over output `oi`, if any, so that a window beneath it can
    /// have the focus.
    fn hide_special(&mut self, si: usize, oi: usize) {
        let seat = &mut self.seats[si];
        if let Some(special) = seat.outputs[oi].special.take() {
            seat.hidden_specials.push(special);
        }
    }

    /// Closes the special workspace in `slot` if its last window just left it, as Hyprland does
    /// (`misc:close_special_on_empty`); one toggled on empty stays until toggled off.
    fn close_special_if_emptied(&mut self, si: usize, slot: Slot) {
        if let Slot::ShownSpecial { output } = slot
            && self.workspace(si, slot).is_empty()
        {
            self.hide_special(si, output);
        }
    }

    /// Removes workspace `index` from output `oi`, first showing another one if it is shown, and
    /// returns it.
    fn take_workspace(&mut self, si: usize, oi: usize, index: usize) -> Workspace {
        let tiling = self.options.default_tiling;
        let output = &mut self.seats[si].outputs[oi];
        if output.workspaces.len() == 1 {
            let id = output.id;
            output
                .workspaces
                .push(Workspace::new(None, Some(id), tiling));
        }
        if self.seats[si].outputs[oi].shown == index {
            let other = if index > 0 { index - 1 } else { index + 1 };
            self.show(si, oi, other);
        }
        let output = &mut self.seats[si].outputs[oi];
        let workspace = output.workspaces.remove(index);
        if output.shown > index {
            output.shown -= 1;
        }
        workspace
    }

    /// Runs after every change: removes the workspaces that have no reason to exist, gives every
    /// output the workspaces it must have, and puts every view back on its focused window.
    fn tidy(&mut self, si: usize) {
        let options = self.options.clone();
        let seat = &mut self.seats[si];
        for output in &mut seat.outputs {
            gather_pinned(output, options.gap);
            let last = output.workspaces.len().saturating_sub(1);
            for i in (0..output.workspaces.len()).rev() {
                let ws = &output.workspaces[i];
                let keep_last_empty = options.dynamic_workspaces && i == last;
                if ws.name.is_none() && ws.is_empty() && i != output.shown && !keep_last_empty {
                    output.workspaces.remove(i);
                    if output.shown > i {
                        output.shown -= 1;
                    }
                }
            }
            let needs_empty_last = options.dynamic_workspaces
                && output
                    .workspaces
                    .last()
                    .is_none_or(|w| w.name.is_some() || !w.is_empty());
            if needs_empty_last || output.workspaces.is_empty() {
                output.workspaces.push(Workspace::new(
                    None,
                    Some(output.id),
                    options.default_tiling,
                ));
            }
            let area = output.area(options.gap);
            for workspace in &mut output.workspaces {
                workspace.fit_view(&area);
            }
            if let Some(special) = &mut output.special {
                special.fit_view(&area);
            }
        }
        seat.hidden_specials.retain(|w| !w.is_empty());
        seat.detached.retain(|w| w.name.is_some() || !w.is_empty());
        if seat.active >= seat.outputs.len() {
            seat.active = 0;
        }
        // A window floats back on leaving fullscreen only through `set_fullscreen`; fullscreen
        // ended any other way (focus moving in dwindle, a window joining its column, a tiling
        // switch) leaves it tiled, so the promise is dropped.
        let seat_id = seat.id;
        let stale: Vec<WindowId> = self
            .windows
            .iter()
            .filter(|(_, r)| r.seat == seat_id && r.float_after_fullscreen)
            .map(|(&w, _)| w)
            .filter(|&w| !self.window(w).is_some_and(|info| info.fullscreen))
            .collect();
        for window in stale {
            if let Some(record) = self.windows.get_mut(&window) {
                record.float_after_fullscreen = false;
            }
        }
    }

    // ----- Seats and outputs -----

    /// Adds a seat with no output.
    pub fn add_seat(&mut self, seat: SeatId) -> Result<(), LayoutError> {
        if self.seats.iter().any(|s| s.id == seat) {
            return Err(LayoutError::AlreadyExists);
        }
        self.seats.push(Seat {
            id: seat,
            outputs: Vec::new(),
            active: 0,
            detached: Vec::new(),
            hidden_specials: Vec::new(),
        });
        Ok(())
    }

    /// Removes a seat and its outputs. Refused while it holds windows: they are closed first.
    pub fn remove_seat(&mut self, seat: SeatId) -> Result<(), LayoutError> {
        let si = self.seat_index(seat)?;
        if self.windows.values().any(|r| r.seat == seat) {
            return Err(LayoutError::NotEmpty);
        }
        self.seats.remove(si);
        Ok(())
    }

    /// Adds an output to a seat. The seat's workspaces whose home is this output come back to it,
    /// unless they are unnamed and empty, and so do the workspaces left without an output; the
    /// one it showed when it was removed is shown again. Without any, it gets an unnamed one. The
    /// seat's first output becomes its active one.
    pub fn add_output(
        &mut self,
        seat: SeatId,
        output: OutputId,
        config: OutputConfig,
    ) -> Result<(), LayoutError> {
        let si = self.seat_index(seat)?;
        if self.output_index(output).is_ok() {
            return Err(LayoutError::AlreadyExists);
        }
        let scale = Scale::new(config.scale).ok_or(LayoutError::InvalidValue)?;
        let position = config.position;
        let physical_ok = |pixels: u32| (1.0..=MAX_SIZE).contains(&f64::from(pixels));
        if !valid_coordinate(position.x)
            || !valid_coordinate(position.y)
            || !physical_ok(config.physical_width)
            || !physical_ok(config.physical_height)
        {
            return Err(LayoutError::InvalidValue);
        }
        let rect = scale.snap_rect(Rect::new(
            position.x,
            position.y,
            f64::from(config.physical_width) / scale.get(),
            f64::from(config.physical_height) / scale.get(),
        ));
        let seat_ref = &mut self.seats[si];
        let mut workspaces: Vec<Workspace> = std::mem::take(&mut seat_ref.detached);
        seat_ref.outputs.push(Output {
            id: output,
            rect,
            scale,
            workspaces: Vec::new(),
            shown: 0,
            special: None,
        });
        let new = seat_ref.outputs.len() - 1;
        // Workspaces at home here come back from the seat's other outputs.
        for oi in 0..new {
            let mut index = 0;
            while index < self.seats[si].outputs[oi].workspaces.len() {
                let workspace = &self.seats[si].outputs[oi].workspaces[index];
                let comes_back = workspace.home == Some(output)
                    && (workspace.name.is_some() || !workspace.is_empty());
                if comes_back {
                    workspaces.push(self.take_workspace(si, oi, index));
                } else {
                    index += 1;
                }
            }
        }
        let area = self.seats[si].outputs[new].area(self.options.gap);
        for workspace in &mut workspaces {
            workspace.fit_view(&area);
        }
        let shown = workspaces
            .iter()
            .position(|w| w.was_shown_at_home && w.home == Some(output))
            .unwrap_or(0);
        for workspace in &mut workspaces {
            workspace.was_shown_at_home = false;
        }
        let new_output = &mut self.seats[si].outputs[new];
        new_output.workspaces = workspaces;
        new_output.shown = shown;
        self.tidy(si);
        Ok(())
    }

    /// Removes an output. Its workspaces move to the seat's active output, or another of its
    /// outputs, keeping this one as their home to come back to; with no output left, they wait,
    /// windows kept. A special workspace shown on it is hidden.
    pub fn remove_output(&mut self, output: OutputId) -> Result<(), LayoutError> {
        let (si, oi) = self.output_index(output)?;
        let gap = self.options.gap;
        let seat = &mut self.seats[si];
        let removed = seat.outputs.remove(oi);
        if seat.active > oi || seat.active >= seat.outputs.len() {
            seat.active = seat.active.saturating_sub(1);
        }
        if let Some(special) = removed.special {
            seat.hidden_specials.push(special);
        }
        let shown = removed.shown;
        let mut workspaces = removed.workspaces;
        if let Some(workspace) = workspaces.get_mut(shown)
            && workspace.home == Some(output)
        {
            workspace.was_shown_at_home = true;
        }
        let Some(target) = seat.outputs.get_mut(seat.active) else {
            seat.detached.append(&mut workspaces);
            self.tidy(si);
            return Ok(());
        };
        // The pinned windows go with them; `tidy` brings them over to the workspace shown.
        let area = target.area(gap);
        for mut workspace in workspaces {
            workspace.fit_view(&area);
            target.workspaces.push(workspace);
        }
        self.tidy(si);
        Ok(())
    }

    /// Makes `output` the seat's active output, where its keys act and its windows open. Refused
    /// for another seat's output.
    pub fn focus_output(&mut self, seat: SeatId, output: OutputId) -> Result<(), LayoutError> {
        let si = self.seat_index(seat)?;
        let (owner, oi) = self.output_index(output)?;
        if owner != si {
            return Err(LayoutError::Refused("the output belongs to another seat"));
        }
        self.seats[si].active = oi;
        Ok(())
    }

    // ----- Workspaces -----

    /// Adds a named workspace whose home is `output`, at the end of that output's list. An
    /// output not yet plugged in is accepted: until it comes, the workspace waits on the seat's
    /// active output (niri's way), or with the workspaces without an output. Without `output`,
    /// the workspace goes to the seat's active output and makes it its home. Refused for another
    /// seat's output.
    pub fn add_workspace(
        &mut self,
        seat: SeatId,
        name: &str,
        output: Option<OutputId>,
    ) -> Result<(), LayoutError> {
        let si = self.seat_index(seat)?;
        if self.named_slot(si, name).is_some() {
            return Err(LayoutError::AlreadyExists);
        }
        let home_index = match output.map(|o| self.output_index(o)) {
            Some(Ok((owner, _))) if owner != si => {
                return Err(LayoutError::Refused("the output belongs to another seat"));
            }
            Some(Ok((_, oi))) => Some(oi),
            Some(Err(_)) | None => None,
        };
        let tiling = self.options.default_tiling;
        let seat_ref = &mut self.seats[si];
        let index = home_index.or((!seat_ref.outputs.is_empty()).then_some(seat_ref.active));
        let home = output.or(index.map(|oi| seat_ref.outputs[oi].id));
        let workspace = Workspace::new(Some(name.to_owned()), home, tiling);
        match index {
            Some(oi) => seat_ref.outputs[oi].workspaces.push(workspace),
            None => seat_ref.detached.push(workspace),
        }
        self.tidy(si);
        Ok(())
    }

    /// Removes a named workspace: it loses its name and keeps its windows, as in niri; it goes once
    /// it is empty and not shown.
    pub fn remove_workspace(&mut self, seat: SeatId, name: &str) -> Result<(), LayoutError> {
        let si = self.seat_index(seat)?;
        let slot = self
            .named_slot(si, name)
            .ok_or_else(|| LayoutError::UnknownWorkspace(name.to_owned()))?;
        let (workspace, _) = self.workspace_mut(si, slot);
        workspace.name = None;
        self.tidy(si);
        Ok(())
    }

    /// Shows the workspace named `name` and makes its output the seat's active one; creates it on
    /// the active output if it does not exist.
    pub fn focus_workspace(&mut self, seat: SeatId, name: &str) -> Result<(), LayoutError> {
        let si = self.seat_index(seat)?;
        let slot = self.named_slot_or_create(si, name);
        self.show_slot(si, slot);
        self.tidy(si);
        Ok(())
    }

    /// Shows the workspace in `slot` and makes its output active; a detached workspace moves to
    /// the active output first, if the seat has one.
    fn show_slot(&mut self, si: usize, slot: Slot) {
        match slot {
            Slot::Output { output, index } => {
                self.show(si, output, index);
                self.seats[si].active = output;
            }
            Slot::Detached(i) => {
                let seat = &mut self.seats[si];
                let active = seat.active;
                if let Some(output) = seat.outputs.get_mut(active) {
                    let mut workspace = seat.detached.remove(i);
                    workspace.settle_on(output.id);
                    output.workspaces.push(workspace);
                    let index = output.workspaces.len() - 1;
                    self.show(si, active, index);
                }
            }
            Slot::ShownSpecial { output } => self.seats[si].active = output,
            Slot::HiddenSpecial(i) => {
                let seat = &mut self.seats[si];
                let active = seat.active;
                if let Some(output) = seat.outputs.get_mut(active) {
                    let special = seat.hidden_specials.remove(i);
                    if let Some(previous) = output.special.replace(special) {
                        seat.hidden_specials.push(previous);
                    }
                }
            }
        }
    }

    /// Shows the previous (`Up`) or next (`Down`) workspace of the seat's active output. Left and
    /// right do nothing: workspaces are stacked vertically.
    pub fn focus_workspace_relative(
        &mut self,
        seat: SeatId,
        direction: Direction,
    ) -> Result<(), LayoutError> {
        let si = self.seat_index(seat)?;
        if let Some(Slot::Output { output, index }) = self.relative_workspace(si, direction) {
            self.show(si, output, index);
            self.tidy(si);
        }
        Ok(())
    }

    /// The workspace above or below the one the seat's active output shows.
    fn relative_workspace(&self, si: usize, direction: Direction) -> Option<Slot> {
        let seat = &self.seats[si];
        let output = seat.outputs.get(seat.active)?;
        let index = match direction {
            Direction::Up => output.shown.checked_sub(1)?,
            Direction::Down => output.shown + 1,
            Direction::Left | Direction::Right => return None,
        };
        (index < output.workspaces.len()).then_some(Slot::Output {
            output: seat.active,
            index,
        })
    }

    /// Shows the special workspace `name` over the seat's active output, focus in it, created if
    /// it does not exist and taken from another output that shows it; hides it if this output
    /// shows it already.
    pub fn toggle_special(&mut self, seat: SeatId, name: &str) -> Result<(), LayoutError> {
        let si = self.seat_index(seat)?;
        let tiling = self.options.default_tiling;
        let seat_ref = &mut self.seats[si];
        let active = seat_ref.active;
        if active >= seat_ref.outputs.len() {
            return Err(LayoutError::Refused("the seat has no output"));
        }
        if seat_ref.outputs[active]
            .special
            .as_ref()
            .is_some_and(|s| s.name.as_deref() == Some(name))
        {
            let special = seat_ref.outputs[active].special.take();
            seat_ref.hidden_specials.extend(special);
        } else {
            let hidden = seat_ref
                .hidden_specials
                .iter()
                .position(|s| s.name.as_deref() == Some(name))
                .map(|i| seat_ref.hidden_specials.remove(i));
            let shown_elsewhere = || {
                seat_ref.outputs.iter_mut().find_map(|o| {
                    if o.special.as_ref()?.name.as_deref() == Some(name) {
                        o.special.take()
                    } else {
                        None
                    }
                })
            };
            let special = hidden
                .or_else(shown_elsewhere)
                .unwrap_or_else(|| Workspace::new(Some(name.to_owned()), None, tiling));
            if let Some(previous) = seat_ref.outputs[active].special.replace(special) {
                seat_ref.hidden_specials.push(previous);
            }
        }
        self.tidy(si);
        Ok(())
    }

    /// Changes the tiling of the workspace the seat works in.
    pub fn set_tiling(&mut self, seat: SeatId, tiling: Tiling) -> Result<(), LayoutError> {
        let si = self.seat_index(seat)?;
        let Some(slot) = self.focused_slot(si) else {
            return Ok(());
        };
        let width = self.options.default_width;
        let (workspace, area) = self.workspace_mut(si, slot);
        workspace.set_tiling(tiling, width, area.as_ref());
        self.tidy(si);
        Ok(())
    }

    // ----- Focus and moves by a seat -----

    /// Applies `change` to the workspace the seat works in, then tidies.
    fn with_focused_workspace(
        &mut self,
        seat: SeatId,
        change: impl FnOnce(&mut Workspace, Option<&Area>),
    ) -> Result<(), LayoutError> {
        let si = self.seat_index(seat)?;
        if let Some(slot) = self.focused_slot(si) {
            let (workspace, area) = self.workspace_mut(si, slot);
            change(workspace, area.as_ref());
            self.tidy(si);
        }
        Ok(())
    }

    /// Moves the seat's focus to the next window in `direction`. In scrolling columns, it stops
    /// at the strip's ends.
    pub fn focus(&mut self, seat: SeatId, direction: Direction) -> Result<(), LayoutError> {
        self.with_focused_workspace(seat, |ws, area| ws.focus(direction, area))
    }

    /// Moves the seat's focused column (left, right) or window (up, down) in scrolling columns;
    /// exchanges the focused window with its neighbour in dwindle. Floating windows do not move.
    pub fn move_focused(&mut self, seat: SeatId, direction: Direction) -> Result<(), LayoutError> {
        self.with_focused_workspace(seat, |ws, area| ws.move_focused(direction, area))
    }

    /// niri's consume-or-expel, left or right, on the seat's focused window in scrolling columns:
    /// a window sharing its column leaves it, a window alone joins its neighbour's column.
    pub fn consume_or_expel(
        &mut self,
        seat: SeatId,
        direction: Direction,
    ) -> Result<(), LayoutError> {
        self.with_focused_workspace(seat, |ws, area| {
            if let (TilingLayout::Scrolling(s), false) = (&mut ws.tiling, ws.focus_floating) {
                s.consume_or_expel(direction, area);
            }
        })
    }

    /// Switches the seat's focus between the floating and the tiled windows.
    pub fn switch_focus_floating(&mut self, seat: SeatId) -> Result<(), LayoutError> {
        self.with_focused_workspace(seat, |ws, _| ws.switch_focus_side())
    }

    /// Moves the seat's focused window to the workspace named `name`, created if it does not
    /// exist; the seat follows it there.
    pub fn move_to_workspace(&mut self, seat: SeatId, name: &str) -> Result<(), LayoutError> {
        let si = self.seat_index(seat)?;
        let Some((source, window)) = self.focused_with_slot(si) else {
            return Ok(());
        };
        let target = self.named_slot_or_create(si, name);
        if target != source {
            self.move_window(si, source, window, target, true);
        }
        self.tidy(si);
        Ok(())
    }

    /// Moves the seat's focused window to the workspace above (`Up`) or below (`Down`) on its
    /// output; the seat follows it there.
    pub fn move_to_workspace_relative(
        &mut self,
        seat: SeatId,
        direction: Direction,
    ) -> Result<(), LayoutError> {
        let si = self.seat_index(seat)?;
        let Some((source, window)) = self.focused_with_slot(si) else {
            return Ok(());
        };
        if matches!(source, Slot::Output { .. })
            && let Some(target) = self.relative_workspace(si, direction)
        {
            self.move_window(si, source, window, target, true);
        }
        self.tidy(si);
        Ok(())
    }

    /// Moves the seat's focused window to the special workspace `name`, created if it does not
    /// exist, without following it (Hyprland's way to put a window aside).
    pub fn move_to_special(&mut self, seat: SeatId, name: &str) -> Result<(), LayoutError> {
        let si = self.seat_index(seat)?;
        let Some((source, window)) = self.focused_with_slot(si) else {
            return Ok(());
        };
        let tiling = self.options.default_tiling;
        let seat_ref = &self.seats[si];
        let shown = seat_ref
            .outputs
            .iter()
            .position(|o| {
                o.special
                    .as_ref()
                    .is_some_and(|s| s.name.as_deref() == Some(name))
            })
            .map(|output| Slot::ShownSpecial { output });
        let hidden = seat_ref
            .hidden_specials
            .iter()
            .position(|s| s.name.as_deref() == Some(name))
            .map(Slot::HiddenSpecial);
        let target = match shown.or(hidden) {
            Some(target) => target,
            None => {
                let seat_ref = &mut self.seats[si];
                seat_ref
                    .hidden_specials
                    .push(Workspace::new(Some(name.to_owned()), None, tiling));
                Slot::HiddenSpecial(seat_ref.hidden_specials.len() - 1)
            }
        };
        if target != source {
            self.move_window(si, source, window, target, false);
        }
        self.tidy(si);
        Ok(())
    }

    /// The workspace the seat works in and its focused window.
    fn focused_with_slot(&self, si: usize) -> Option<(Slot, WindowId)> {
        let slot = self.focused_slot(si)?;
        let window = self.workspace(si, slot).focused()?;
        Some((slot, window))
    }

    /// Moves `window` from `source` to `target` in the same seat, keeping it floating or tiled,
    /// fullscreen, and its column's width and full width; a pinned window is unpinned. With
    /// `follow`, the seat shows the target and focuses the window.
    fn move_window(
        &mut self,
        si: usize,
        source: Slot,
        window: WindowId,
        target: Slot,
        follow: bool,
    ) {
        let (from, area) = self.workspace_mut(si, source);
        let fullscreen = from.is_fullscreen(window);
        let column = from.column_width(window);
        let Some(removed) = from.remove(window, area.as_ref()) else {
            return;
        };
        let moving = Moving {
            window,
            floating: removed.map(|(rect, _)| rect),
            fullscreen,
            column,
        };
        self.insert_window(si, target, moving);
        self.close_special_if_emptied(si, source);
        if follow {
            if let Slot::Output { output, .. } = target {
                self.hide_special(si, output);
            }
            self.show_slot(si, target);
            // Showing a detached workspace moves it: find the window again.
            if let Ok((_, slot)) = self.find_window(window) {
                let (workspace, area) = self.workspace_mut(si, slot);
                workspace.focus_window(window, area.as_ref());
            }
        }
    }

    /// Puts a window into the workspace in `slot`, as it was before it moved.
    fn insert_window(&mut self, si: usize, slot: Slot, moving: Moving) {
        let options = self.options.clone();
        let home = match slot {
            Slot::Output { output, .. } => Some(self.seats[si].outputs[output].id),
            _ => None,
        };
        let (workspace, area) = self.workspace_mut(si, slot);
        if let Some(home) = home {
            workspace.settle_on(home);
        }
        match moving.floating {
            Some(rect) => {
                workspace.add_floating(moving.window, Some(rect), rect.size(), area.as_ref(), true);
            }
            None => {
                let (width, full_width) = moving.column.unwrap_or((options.default_width, false));
                workspace.add_tiled(moving.window, width, area.as_ref());
                if let (true, TilingLayout::Scrolling(s)) = (full_width, &mut workspace.tiling) {
                    s.set_full_width(moving.window, true, area.as_ref());
                }
                if moving.fullscreen {
                    set_tiled_fullscreen(workspace, moving.window, true, area.as_ref());
                }
            }
        }
    }

    // ----- Windows -----

    /// Opens a window for a seat: on the named workspace of `options`, else on the special
    /// workspace shown on the seat's active output, else on the workspace that output shows. A
    /// seat without an output keeps it on a workspace that waits for one.
    pub fn open_window(
        &mut self,
        seat: SeatId,
        window: WindowId,
        options: OpenOptions,
    ) -> Result<(), LayoutError> {
        let si = self.seat_index(seat)?;
        if self.windows.contains_key(&window) {
            return Err(LayoutError::AlreadyExists);
        }
        if let Some(size) = options.size
            && !valid_size(size)
        {
            return Err(LayoutError::InvalidValue);
        }
        if let Some(width) = options.width
            && !valid_width(width)
        {
            return Err(LayoutError::InvalidValue);
        }
        let slot = match &options.workspace {
            Some(name) => self.named_slot_or_create(si, name),
            None => match self.focused_slot(si) {
                Some(slot) => slot,
                None => {
                    let tiling = self.options.default_tiling;
                    let seat_ref = &mut self.seats[si];
                    if seat_ref.detached.is_empty() {
                        seat_ref.detached.push(Workspace::new(None, None, tiling));
                    }
                    Slot::Detached(0)
                }
            },
        };
        self.windows.insert(
            window,
            WindowRecord {
                seat,
                fake_fullscreen: false,
                floating_rect: None,
                // As niri: a window opened floating and fullscreen floats on leaving fullscreen.
                float_after_fullscreen: options.floating && options.fullscreen,
            },
        );
        let home = match slot {
            Slot::Output { output, .. } => Some(self.seats[si].outputs[output].id),
            _ => None,
        };
        let default_width = self.options.default_width;
        let default_size = self.options.default_floating_size;
        let (workspace, area) = self.workspace_mut(si, slot);
        if let Some(home) = home {
            workspace.settle_on(home);
        }
        if options.floating && !options.fullscreen {
            let size = options.size.unwrap_or(default_size);
            workspace.add_floating(window, None, size, area.as_ref(), true);
        } else {
            let width = options.width.unwrap_or(default_width);
            workspace.add_tiled(window, width, area.as_ref());
            if options.fullscreen {
                set_tiled_fullscreen(workspace, window, true, area.as_ref());
            }
        }
        self.tidy(si);
        Ok(())
    }

    /// Closes a window: it leaves the model.
    pub fn close_window(&mut self, window: WindowId) -> Result<(), LayoutError> {
        let (si, slot) = self.find_window(window)?;
        let (workspace, area) = self.workspace_mut(si, slot);
        workspace.remove(window, area.as_ref());
        self.windows.remove(&window);
        self.close_special_if_emptied(si, slot);
        self.tidy(si);
        Ok(())
    }

    /// Gives a window its seat's focus, showing its workspace and making its output active. A
    /// hidden special workspace is shown on the seat's active output; a special workspace shown
    /// over the window's own is hidden. Refused for a window on a workspace without an output.
    pub fn focus_window(&mut self, window: WindowId) -> Result<(), LayoutError> {
        let (si, slot) = self.find_window(window)?;
        if matches!(slot, Slot::Detached(_)) || self.seats[si].outputs.is_empty() {
            return Err(LayoutError::Refused("the window's workspace has no output"));
        }
        if let Slot::Output { output, .. } = slot {
            self.hide_special(si, output);
        }
        self.show_slot(si, slot);
        let (_, slot) = self.find_window(window)?;
        let (workspace, area) = self.workspace_mut(si, slot);
        workspace.focus_window(window, area.as_ref());
        self.tidy(si);
        Ok(())
    }

    /// Floats a tiled window, at the rectangle it last floated at or centred at its tiled size;
    /// or tiles a floating window, where a new window would go. A fullscreen window leaves
    /// fullscreen first; a pinned one is unpinned.
    pub fn set_floating(&mut self, window: WindowId, floating: bool) -> Result<(), LayoutError> {
        let (si, slot) = self.find_window(window)?;
        let default_width = self.options.default_width;
        let default_size = self.options.default_floating_size;
        let saved_rect = self.windows.get(&window).and_then(|r| r.floating_rect);
        let (workspace, area) = self.workspace_mut(si, slot);
        let is_tiled = workspace.is_tiled(window);
        let mut new_rect = None;
        if floating && is_tiled {
            // Out of fullscreen first, so that its tiled size, not the output's, is measured.
            set_tiled_fullscreen(workspace, window, false, area.as_ref());
            let size = area
                .as_ref()
                .and_then(|area| tiled_rect(workspace, window, area))
                .map_or(default_size, |r| r.size());
            let was_focused = workspace.focused() == Some(window);
            workspace.remove(window, area.as_ref());
            workspace.add_floating(window, saved_rect, size, area.as_ref(), was_focused);
        } else if !floating && !is_tiled {
            let was_focused = workspace.focused() == Some(window);
            if let Some(Some((rect, _))) = workspace.remove(window, area.as_ref()) {
                new_rect = Some(rect);
            }
            workspace.add_tiled(window, default_width, area.as_ref());
            if was_focused {
                workspace.focus_window(window, area.as_ref());
            }
        }
        if let (Some(rect), Some(record)) = (new_rect, self.windows.get_mut(&window)) {
            record.floating_rect = Some(rect);
            record.float_after_fullscreen = false;
        }
        self.tidy(si);
        Ok(())
    }

    /// Moves and resizes a floating window; `rect` is relative to the working area's top-left
    /// corner, and the window is kept inside it. Refused for a tiled window.
    pub fn set_floating_rect(&mut self, window: WindowId, rect: Rect) -> Result<(), LayoutError> {
        if !(valid_coordinate(rect.x) && valid_coordinate(rect.y) && valid_size(rect.size())) {
            return Err(LayoutError::InvalidValue);
        }
        let (si, slot) = self.find_window(window)?;
        let (workspace, area) = self.workspace_mut(si, slot);
        if !workspace.floating.set_rect(window, rect, area.as_ref()) {
            return Err(LayoutError::Refused("the window is tiled"));
        }
        self.tidy(si);
        Ok(())
    }

    /// Makes a window fullscreen, or not. A fullscreen window is always tiled, as in niri: a
    /// floating one moves into the tiled layout, and floats again where it was when it leaves
    /// fullscreen. Either way the window stops being fake fullscreen: the two states exclude each
    /// other, and a client asking to leave fullscreen leaves both (niri). Refused for a pinned
    /// window, as in Hyprland: unpin it first.
    pub fn set_fullscreen(
        &mut self,
        window: WindowId,
        fullscreen: bool,
    ) -> Result<(), LayoutError> {
        let (si, slot) = self.find_window(window)?;
        if fullscreen && self.workspace(si, slot).floating.is_pinned(window) {
            return Err(LayoutError::Refused("a pinned window cannot be fullscreen"));
        }
        let default_width = self.options.default_width;
        let record = self.windows.get(&window).cloned();
        let (workspace, area) = self.workspace_mut(si, slot);
        let mut float_after = record.as_ref().is_some_and(|r| r.float_after_fullscreen);
        let mut saved_rect = record.as_ref().and_then(|r| r.floating_rect);
        if fullscreen {
            if !workspace.is_fullscreen(window) {
                // As niri: whether it floats back is decided at each entry into fullscreen.
                float_after = !workspace.is_tiled(window);
            }
            if !workspace.is_tiled(window) {
                if let Some(Some((rect, _))) = workspace.remove(window, area.as_ref()) {
                    saved_rect = Some(rect);
                }
                workspace.add_tiled(window, default_width, area.as_ref());
            }
            set_tiled_fullscreen(workspace, window, true, area.as_ref());
        } else if workspace.is_fullscreen(window) {
            set_tiled_fullscreen(workspace, window, false, area.as_ref());
            if float_after {
                let was_focused = workspace.focused() == Some(window);
                workspace.remove(window, area.as_ref());
                let size = saved_rect.map_or(self.options.default_floating_size, |r| r.size());
                let (workspace, area) = self.workspace_mut(si, slot);
                workspace.add_floating(window, saved_rect, size, area.as_ref(), was_focused);
                float_after = false;
            }
        }
        if let Some(record) = self.windows.get_mut(&window) {
            record.float_after_fullscreen = float_after;
            record.floating_rect = saved_rect;
            record.fake_fullscreen = false;
        }
        self.tidy(si);
        Ok(())
    }

    /// Tells a window it is fullscreen, or not, without changing its place (fake fullscreen). A
    /// really fullscreen window leaves fullscreen first (niri).
    pub fn set_fake_fullscreen(&mut self, window: WindowId, fake: bool) -> Result<(), LayoutError> {
        let (si, slot) = self.find_window(window)?;
        if fake && self.workspace(si, slot).is_fullscreen(window) {
            self.set_fullscreen(window, false)?;
        }
        if let Some(record) = self.windows.get_mut(&window) {
            record.fake_fullscreen = fake;
        }
        Ok(())
    }

    /// Makes a tiled window's column full width (scrolling), or maximises it over the working
    /// area (dwindle), or neither. Nothing happens to a floating window.
    pub fn set_full_width(
        &mut self,
        window: WindowId,
        full_width: bool,
    ) -> Result<(), LayoutError> {
        let (si, slot) = self.find_window(window)?;
        let (workspace, area) = self.workspace_mut(si, slot);
        match &mut workspace.tiling {
            TilingLayout::Scrolling(s) => {
                s.set_full_width(window, full_width, area.as_ref());
            }
            TilingLayout::Dwindle(d) => {
                d.set_maximized(window, full_width);
            }
        }
        if workspace.is_tiled(window) && workspace.focused_tiled() == Some(window) {
            workspace.focus_floating = false;
        }
        self.tidy(si);
        Ok(())
    }

    /// Sets a tiled window's column to the next preset width (`forward`) or the previous one.
    /// Only scrolling columns on an output have widths; elsewhere nothing happens.
    pub fn cycle_preset_width(
        &mut self,
        window: WindowId,
        forward: bool,
    ) -> Result<(), LayoutError> {
        let (si, slot) = self.find_window(window)?;
        let presets = self.options.preset_widths.clone();
        let (workspace, area) = self.workspace_mut(si, slot);
        if let (TilingLayout::Scrolling(s), Some(area)) = (&mut workspace.tiling, area) {
            s.cycle_preset_width(window, &presets, forward, &area);
        }
        self.tidy(si);
        Ok(())
    }

    /// Widens a tiled window's column by `delta`, a share of the working area; negative narrows.
    /// Only scrolling columns on an output have widths; elsewhere nothing happens.
    pub fn change_width(&mut self, window: WindowId, delta: f64) -> Result<(), LayoutError> {
        if !delta.is_finite() {
            return Err(LayoutError::InvalidValue);
        }
        let (si, slot) = self.find_window(window)?;
        let (workspace, area) = self.workspace_mut(si, slot);
        if let (TilingLayout::Scrolling(s), Some(area)) = (&mut workspace.tiling, area) {
            s.change_width(window, delta, &area);
        }
        self.tidy(si);
        Ok(())
    }

    /// Pins a floating window over every workspace of its output, or unpins it. Refused, as in
    /// Hyprland, for a tiled window (fullscreen ones included), and for a window that is not on the
    /// workspace its output shows (a special one, a hidden one).
    pub fn set_pinned(&mut self, window: WindowId, pinned: bool) -> Result<(), LayoutError> {
        let (si, slot) = self.find_window(window)?;
        let Slot::Output { output, index } = slot else {
            return Err(LayoutError::Refused(
                "only a window of a shown workspace can be pinned",
            ));
        };
        if self.seats[si].outputs[output].shown != index {
            return Err(LayoutError::Refused(
                "only a window of a shown workspace can be pinned",
            ));
        }
        let (workspace, _) = self.workspace_mut(si, slot);
        if !workspace.floating.set_pinned(window, pinned) {
            return Err(LayoutError::Refused("only a floating window can be pinned"));
        }
        self.tidy(si);
        Ok(())
    }

    // ----- Reading the model -----

    /// The window a seat focuses: on its active output, in the special workspace shown there or
    /// in the workspace it shows.
    pub fn focused(&self, seat: SeatId) -> Option<WindowId> {
        let si = self.seat_index(seat).ok()?;
        self.focused_with_slot(si).map(|(_, window)| window)
    }

    /// The seat's active output.
    pub fn active_output(&self, seat: SeatId) -> Option<OutputId> {
        let seat = &self.seats[self.seat_index(seat).ok()?];
        seat.outputs.get(seat.active).map(|o| o.id)
    }

    /// A seat's outputs, in the order they were added.
    pub fn outputs(&self, seat: SeatId) -> Result<Vec<OutputId>, LayoutError> {
        let si = self.seat_index(seat)?;
        Ok(self.seats[si].outputs.iter().map(|o| o.id).collect())
    }

    /// The output's rectangle, in logical coordinates.
    pub fn output_rect(&self, output: OutputId) -> Result<Rect, LayoutError> {
        let (si, oi) = self.output_index(output)?;
        Ok(self.seats[si].outputs[oi].rect)
    }

    /// The windows an output shows, in drawing order, bottom to top: the workspace it shows, its
    /// tiled windows then its floating ones; the special workspace over it; the pinned windows
    /// over everything, as in Hyprland. While a fullscreen window holds a workspace's focus, its
    /// floating and pinned windows are not drawn.
    pub fn arrange(&self, output: OutputId) -> Result<Vec<Placed>, LayoutError> {
        let (si, oi) = self.output_index(output)?;
        let seat = &self.seats[si];
        let o = &seat.outputs[oi];
        let area = o.area(self.options.gap);
        let focused = self.focused(seat.id);
        let place = |a: Arranged| Placed {
            window: a.window,
            rect: a.rect,
            tiled: a.tiled,
            fullscreen: a.fullscreen
                || self
                    .windows
                    .get(&a.window)
                    .is_some_and(|r| r.fake_fullscreen),
            activated: focused == Some(a.window),
        };
        let mut raw = Vec::new();
        o.workspaces[o.shown].arrange(&area, &mut raw);
        let (pinned, unpinned): (Vec<_>, Vec<_>) = raw.into_iter().partition(|a| a.pinned);
        let mut placed: Vec<Placed> = unpinned.into_iter().map(place).collect();
        let mut special_covers = false;
        if let Some(special) = &o.special {
            let mut raw = Vec::new();
            special.arrange(&area, &mut raw);
            placed.extend(raw.into_iter().map(place));
            special_covers = special.fullscreen_covers();
        }
        // A fullscreen window of the special workspace hides them too: nothing over a game.
        if !special_covers {
            placed.extend(pinned.into_iter().map(place));
        }
        Ok(placed)
    }

    /// Where a window is and its state.
    pub fn window(&self, window: WindowId) -> Option<WindowInfo> {
        let (si, slot) = self.find_window(window).ok()?;
        let seat = &self.seats[si];
        let record = self.windows.get(&window)?;
        let workspace = self.workspace(si, slot);
        let (output, special) = match slot {
            Slot::Output { output, .. } => (Some(seat.outputs[output].id), false),
            Slot::ShownSpecial { output } => (Some(seat.outputs[output].id), true),
            Slot::Detached(_) => (None, false),
            Slot::HiddenSpecial(_) => (None, true),
        };
        Some(WindowInfo {
            seat: seat.id,
            output,
            workspace: workspace.name.clone(),
            special,
            floating: workspace.floating.contains(window),
            fullscreen: workspace.is_fullscreen(window),
            fake_fullscreen: record.fake_fullscreen,
            full_width: workspace.is_full_width(window),
            pinned: workspace.floating.is_pinned(window),
        })
    }

    /// The workspaces of an output, in order.
    pub fn workspaces(&self, output: OutputId) -> Result<Vec<WorkspaceInfo>, LayoutError> {
        let (si, oi) = self.output_index(output)?;
        let o = &self.seats[si].outputs[oi];
        Ok(o.workspaces
            .iter()
            .enumerate()
            .map(|(i, w)| WorkspaceInfo {
                name: w.name.clone(),
                shown: i == o.shown,
                tiling: w.tiling_kind(),
                windows: w.windows(),
            })
            .collect())
    }

    /// The name of the special workspace shown over an output.
    pub fn shown_special(&self, output: OutputId) -> Result<Option<String>, LayoutError> {
        let (si, oi) = self.output_index(output)?;
        Ok(self.seats[si].outputs[oi]
            .special
            .as_ref()
            .and_then(|s| s.name.clone()))
    }

    /// Checks every rule the model keeps, and says which one is broken. Its cost grows with the
    /// number of windows and workspaces; it is meant for tests.
    pub fn check_invariants(&self) -> Result<(), String> {
        let mut seen: HashMap<WindowId, SeatId> = HashMap::new();
        for seat in &self.seats {
            let mut names: Vec<&str> = Vec::new();
            let mut special_names: Vec<&str> = Vec::new();
            if !seat.outputs.is_empty() && seat.active >= seat.outputs.len() {
                return Err(format!(
                    "seat {:?}'s active output is out of range",
                    seat.id
                ));
            }
            for output in &seat.outputs {
                let area = output.area(self.options.gap);
                self.check_output(output, &area)?;
                for (i, ws) in output.workspaces.iter().enumerate() {
                    names.extend(ws.name.as_deref());
                    // `tidy` fits the view of every workspace of an output, shown or not.
                    ws.check(Some(&area))
                        .map_err(|e| format!("workspace {i} of output {:?}: {e}", output.id))?;
                    if i != output.shown && ws.floating.pinned().next().is_some() {
                        return Err("a pinned window is on a workspace not shown".to_owned());
                    }
                    record_windows(&mut seen, seat.id, ws)?;
                }
                if let Some(special) = &output.special {
                    special_names.extend(special.name.as_deref());
                    special.check(Some(&area))?;
                    if special.floating.pinned().next().is_some() {
                        return Err("a pinned window is on a special workspace".to_owned());
                    }
                    record_windows(&mut seen, seat.id, special)?;
                }
            }
            if !seat.outputs.is_empty() && !seat.detached.is_empty() {
                return Err(format!(
                    "seat {:?} has outputs and workspaces without one",
                    seat.id
                ));
            }
            for ws in &seat.detached {
                names.extend(ws.name.as_deref());
                ws.check(None)?;
                record_windows(&mut seen, seat.id, ws)?;
            }
            for ws in &seat.hidden_specials {
                special_names.extend(ws.name.as_deref());
                if ws.is_empty() {
                    return Err("an empty special workspace is kept hidden".to_owned());
                }
                if ws.floating.pinned().next().is_some() {
                    return Err("a pinned window is on a special workspace".to_owned());
                }
                ws.check(None)?;
                record_windows(&mut seen, seat.id, ws)?;
            }
            for list in [&mut names, &mut special_names] {
                let count = list.len();
                list.sort_unstable();
                list.dedup();
                if list.len() != count {
                    return Err(format!("seat {:?} has two workspaces of one name", seat.id));
                }
            }
        }
        if seen.len() != self.windows.len() {
            return Err(format!(
                "{} windows are placed, {} are known",
                seen.len(),
                self.windows.len()
            ));
        }
        for (window, seat) in &seen {
            match self.windows.get(window) {
                Some(record) if record.seat == *seat => {}
                _ => return Err(format!("window {window:?} is placed on the wrong seat")),
            }
        }
        for seat in &self.seats {
            let (Some(focused), Some(output)) =
                (self.focused(seat.id), self.active_output(seat.id))
            else {
                continue;
            };
            let drawn = self.arrange(output).map_err(|e| e.to_string())?;
            if !drawn.iter().any(|p| p.window == focused && p.activated) {
                return Err(format!(
                    "seat {:?} focuses {focused:?}, which is not drawn",
                    seat.id
                ));
            }
        }
        for (window, record) in &self.windows {
            if record.float_after_fullscreen && !self.window(*window).is_some_and(|i| i.fullscreen)
            {
                return Err(format!(
                    "{window:?} would float back, but it is not fullscreen"
                ));
            }
        }
        Ok(())
    }

    fn check_output(&self, output: &Output, area: &Area) -> Result<(), String> {
        let workspaces = &output.workspaces;
        if workspaces.is_empty() || output.shown >= workspaces.len() {
            return Err(format!(
                "output {:?} shows no workspace of its own",
                output.id
            ));
        }
        let last = workspaces.len() - 1;
        for (i, ws) in workspaces.iter().enumerate() {
            let allowed = i == output.shown || (self.options.dynamic_workspaces && i == last);
            if ws.name.is_none() && ws.is_empty() && !allowed {
                return Err(format!(
                    "output {:?} keeps empty unnamed workspace {i}",
                    output.id
                ));
            }
        }
        if self.options.dynamic_workspaces {
            let last = &workspaces[last];
            if last.name.is_some() || !last.is_empty() {
                return Err(format!(
                    "output {:?} has no empty workspace last",
                    output.id
                ));
            }
        }
        let placed = self
            .arrange(output.id)
            .map_err(|e| format!("arranging output {:?}: {e}", output.id))?;
        self.check_placed(output, area, &placed)
    }

    /// Checks what an output shows: rectangles on the grid; fullscreen windows over the output;
    /// tiled windows apart from one another, within the working area's height.
    fn check_placed(&self, output: &Output, area: &Area, placed: &[Placed]) -> Result<(), String> {
        let shown = &output.workspaces[output.shown];
        let layers: [(&Workspace, Vec<&Placed>); 2] = [
            (
                shown,
                placed.iter().filter(|p| shown.contains(p.window)).collect(),
            ),
            match &output.special {
                Some(special) => (
                    special,
                    placed
                        .iter()
                        .filter(|p| special.contains(p.window))
                        .collect(),
                ),
                None => (shown, Vec::new()),
            },
        ];
        for p in placed {
            if !area.scale.is_rect_on_grid(&p.rect) {
                return Err(format!(
                    "{:?} at {:?} is off the physical grid",
                    p.window, p.rect
                ));
            }
        }
        for (workspace, placed) in layers {
            let tiled: Vec<&&Placed> = placed
                .iter()
                .filter(|p| p.tiled && !workspace.is_maximized(p.window))
                .collect();
            for p in &tiled {
                if workspace.is_fullscreen(p.window) {
                    if (p.rect.y - area.output.y).abs() > 1e-6
                        || (p.rect.w - area.output.w).abs() > 1e-6
                        || (p.rect.h - area.output.h).abs() > 1e-6
                    {
                        return Err(format!("fullscreen {:?} is at {:?}", p.window, p.rect));
                    }
                } else if p.rect.y < area.working.y - 1e-6
                    || p.rect.bottom() > area.working.bottom() + 1e-6
                {
                    return Err(format!("tiled {:?} leaves the working area", p.window));
                }
            }
            for (i, a) in tiled.iter().enumerate() {
                for b in &tiled[i + 1..] {
                    if a.rect.overlaps(&b.rect) {
                        return Err(format!("tiled {:?} and {:?} overlap", a.window, b.window));
                    }
                }
            }
        }
        Ok(())
    }
}

/// Brings the pinned windows of an output's workspaces over to the workspace it shows, on top,
/// without taking the focus: a pinned window shows over whichever workspace its output shows.
fn gather_pinned(output: &mut Output, gap: f64) {
    let area = output.area(gap);
    let shown = output.shown;
    for index in 0..output.workspaces.len() {
        if index == shown {
            continue;
        }
        let pinned: Vec<WindowId> = output.workspaces[index].floating.pinned().collect();
        for window in pinned {
            if let Some(Some((rect, _))) = output.workspaces[index].remove(window, Some(&area)) {
                let to = &mut output.workspaces[shown];
                to.add_floating(window, Some(rect), rect.size(), Some(&area), false);
                to.floating.set_pinned(window, true);
            }
        }
    }
}

/// Records the windows of a workspace as placed on `seat`; a window placed twice is an error.
fn record_windows(
    seen: &mut HashMap<WindowId, SeatId>,
    seat: SeatId,
    workspace: &Workspace,
) -> Result<(), String> {
    for window in workspace.windows() {
        if seen.insert(window, seat).is_some() {
            return Err(format!("window {window:?} is placed twice"));
        }
    }
    Ok(())
}

/// Makes a tiled window fullscreen or not in whichever layout holds it, focus kept on the floating
/// side only while no fullscreen window covers.
fn set_tiled_fullscreen(
    workspace: &mut Workspace,
    window: WindowId,
    on: bool,
    area: Option<&Area>,
) {
    match &mut workspace.tiling {
        TilingLayout::Scrolling(s) => {
            s.set_fullscreen(window, on, area);
        }
        TilingLayout::Dwindle(d) => {
            d.set_fullscreen(window, on);
        }
    }
    if on && workspace.focused_tiled() == Some(window) {
        workspace.focus_floating = false;
    }
}

/// A tiled window's rectangle on `area`.
fn tiled_rect(workspace: &Workspace, window: WindowId, area: &Area) -> Option<Rect> {
    let mut placed = Vec::new();
    workspace.arrange(area, &mut placed);
    placed
        .into_iter()
        .find(|a| a.window == window)
        .map(|a| a.rect)
}

/// The widest size, in logical units, a window or an output may have: 25 times a 4K output's
/// width at scale 1. Beyond it, numbers mean nothing and only push other windows away.
const MAX_SIZE: f64 = 100_000.0;
/// The farthest an output or a floating window may lie from the origin, in logical units.
const MAX_COORDINATE: f64 = 1_000_000.0;
/// The widest gap.
const MAX_GAP: f64 = 1_000.0;

fn valid_size(size: Size) -> bool {
    (f64::MIN_POSITIVE..=MAX_SIZE).contains(&size.w)
        && (f64::MIN_POSITIVE..=MAX_SIZE).contains(&size.h)
}

fn valid_coordinate(value: f64) -> bool {
    (-MAX_COORDINATE..=MAX_COORDINATE).contains(&value)
}

/// A proportion between [`ColumnWidth::MIN_PROPORTION`] and 1, or a fixed width up to
/// [`MAX_SIZE`].
fn valid_width(width: ColumnWidth) -> bool {
    match width {
        ColumnWidth::Proportion(share) => (ColumnWidth::MIN_PROPORTION..=1.0).contains(&share),
        ColumnWidth::Fixed(width) => (f64::MIN_POSITIVE..=MAX_SIZE).contains(&width),
    }
}
