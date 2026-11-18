//! An output's workspace switch: the workspace left slides out and the one shown slides in,
//! along [ADR-0018](../../../../docs/adr/0018-workspaces.md)'s vertical stack.

use std::time::Duration;

use yuna_layout::WorkspaceId;

use crate::{Spring, SpringParams};

/// A switch between an output's workspaces.
///
/// Only the workspaces it involves are in its stack, in the order the output lists them: the
/// one left and the one shown, and those a retarget went through, so that nothing between them
/// crosses the view ([ADR-0055](../../../../docs/adr/0055-what-moves-first-and-animations-off-headless.md)).
/// Each sits on a row, that many output heights down, the rows growing along the stack. No
/// workspace ever changes row, so that nothing drawn of it jumps: one that leaves the output
/// leaves its row empty, and one shown between two on rows next to each other takes the place
/// of the one farther from the view, which leaves the stack. The view's position along the stack
/// is a spring, whose target is the shown workspace's row. The output decides how long it draws
/// the switch: until its slide ends, and the motions of the workspaces it slides.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Switch {
    /// The workspaces involved that are still on the output, in its order.
    stack: Vec<Stacked>,
    /// The top edge of the view along the stack, in logical units.
    position: Spring,
    /// The output's height: how far apart two rows are.
    height: f64,
}

/// A workspace of a switch's stack, on its row.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Stacked {
    workspace: WorkspaceId,
    /// How many output heights down the workspace sits: 0 for the first of a new switch, below 0
    /// for one a retarget put above it.
    row: i64,
}

/// What a switch needs to start: the output's height, its tolerance and the spring's parameters.
#[derive(Debug, Clone, Copy)]
pub(super) struct Slide {
    /// The output's height, in logical units.
    pub height: f64,
    /// Half a physical pixel of the output, in logical units.
    pub tolerance: f64,
    /// The switch's spring.
    pub params: SpringParams,
}

impl Switch {
    /// The switch after an update that found `shown` on the output, which showed `was_shown`
    /// before; `order` gives the output's workspaces, in order, and `drawn` is the switch the
    /// output drew at `now`, if it drew one. `None` when there is no switch to draw.
    ///
    /// A new switch starts when the shown workspace changed and the one left is still on the
    /// output. A switch drawn drops the workspaces gone from the output, takes the newly shown
    /// workspace into its stack, and slides on toward it from where it is, keeping its velocity.
    /// A switch with nowhere to go ends. `order` is called only when the shown workspace changed
    /// or a switch is drawn: reading it lists every workspace's windows.
    pub fn follow(
        drawn: Option<&Switch>,
        was_shown: WorkspaceId,
        shown: WorkspaceId,
        order: impl FnOnce() -> Vec<WorkspaceId>,
        now: Duration,
        slide: Slide,
    ) -> Option<Switch> {
        if drawn.is_none() && was_shown == shown {
            return None;
        }
        let order = order();
        match drawn {
            Some(switch) => switch.restacked(&order, shown, now),
            None if order.contains(&was_shown) => {
                let stack: Vec<Stacked> = order
                    .iter()
                    .filter(|&&workspace| workspace == was_shown || workspace == shown)
                    .zip(0..)
                    .map(|(&workspace, row)| Stacked { workspace, row })
                    .collect();
                let place = |workspace| Some(row_of(&stack, workspace)? as f64 * slide.height);
                let position = Spring::new(
                    slide.params,
                    place(was_shown)?,
                    place(shown)?,
                    0.0,
                    now,
                    slide.tolerance,
                );
                Some(Switch {
                    stack,
                    position,
                    height: slide.height,
                })
            }
            None => None,
        }
    }

    /// This switch after an update that found the output's workspaces in `order`, sliding
    /// toward `shown` from `now`, keeping its velocity.
    ///
    /// No workspace of the stack ever changes row, so that nothing drawn of it jumps: a
    /// workspace gone from the output leaves its row empty, and `shown`, if new to the stack,
    /// takes a row of its own ([`insert`]), or the place of a workspace that leaves the stack.
    /// `None` when no workspace of the stack is still on the output, or when `shown` is not on
    /// it.
    fn restacked(
        &self,
        order: &[WorkspaceId],
        shown: WorkspaceId,
        now: Duration,
    ) -> Option<Switch> {
        let mut stack: Vec<Stacked> = self
            .stack
            .iter()
            .copied()
            .filter(|stacked| order.contains(&stacked.workspace))
            .collect();
        if row_of(&stack, shown).is_none() {
            let view = self.position.value(now) / self.height;
            insert(&mut stack, order, shown, view)?;
        }
        let target = row_of(&stack, shown)? as f64 * self.height;
        let position = if self.position.target() == target {
            self.position
        } else {
            self.position.retarget(now, target)
        };
        Some(Switch {
            stack,
            position,
            height: self.height,
        })
    }

    /// The workspaces involved, in the output's order.
    pub fn workspaces(&self) -> impl Iterator<Item = WorkspaceId> + '_ {
        self.stack.iter().map(|stacked| stacked.workspace)
    }

    /// Each workspace involved, in the output's order, and how far down it is drawn at `at`, in
    /// logical units.
    pub fn slides(&self, at: Duration) -> impl Iterator<Item = (WorkspaceId, f64)> + '_ {
        let position = self.position.value(at);
        self.stack.iter().map(move |stacked| {
            let slide = stacked.row as f64 * self.height - position;
            (stacked.workspace, slide)
        })
    }

    /// When the slide arrives.
    pub fn end(&self) -> Duration {
        self.position.end()
    }
}

/// Puts `shown`, which `stack` does not hold, between the workspaces before and after it in
/// the output's `order`: on the row after the one before it, or on the row above the first.
/// When that row is taken, the two around it are on rows next to each other, and `shown` takes
/// the place of the one farther from `view`, the view's position in rows; that one leaves the
/// stack, and the screen. Moving it a row away instead would make whatever of it shows jump: a
/// window taller than its place, or carried past its workspace's edge by its offset. On a tie,
/// the one after `shown` leaves. `None`, changing nothing, when `stack` is empty or `shown` is
/// not in `order`.
fn insert(
    stack: &mut Vec<Stacked>,
    order: &[WorkspaceId],
    shown: WorkspaceId,
    view: f64,
) -> Option<()> {
    let rank = |workspace| order.iter().position(|&w| w == workspace);
    let shown_rank = rank(shown)?;
    // An output's workspaces keep their order (the layout only adds at the end and removes), so
    // the stack's ranks grow.
    let at = stack.partition_point(|stacked| rank(stacked.workspace) < Some(shown_rank));
    let before = at.checked_sub(1);
    let row = match before {
        Some(before) => stack[before].row + 1,
        None => stack.first()?.row - 1,
    };
    if let Some(before) = before
        && stack.get(at).is_some_and(|next| next.row == row)
    {
        let distance = |index: usize| (stack[index].row as f64 - view).abs();
        let farther = if distance(before) > distance(at) {
            before
        } else {
            at
        };
        stack[farther].workspace = shown;
        return Some(());
    }
    stack.insert(
        at,
        Stacked {
            workspace: shown,
            row,
        },
    );
    Some(())
}

/// The row of `workspace` in `stack`.
fn row_of(stack: &[Stacked], workspace: WorkspaceId) -> Option<i64> {
    stack
        .iter()
        .find(|stacked| stacked.workspace == workspace)
        .map(|stacked| stacked.row)
}
