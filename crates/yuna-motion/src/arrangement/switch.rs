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
/// The workspace at index `i` of the stack sits `i` output heights down; the view's position
/// along the stack is a spring, whose target is the shown workspace's. The output decides how
/// long it draws the switch: until its slide ends, and the motions of the workspaces it slides.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Switch {
    /// The workspaces involved, in the output's order.
    stack: Vec<WorkspaceId>,
    /// The top edge of the view along the stack, in logical units.
    position: Spring,
    /// The output's height: how far apart the workspaces of the stack are.
    height: f64,
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
    /// output. A switch drawn takes the newly shown workspace into its stack, drops the
    /// workspaces gone from the output, and slides on toward the shown one from where it is,
    /// keeping its velocity. A switch with nowhere to go ends. `order` is called only when the
    /// shown workspace changed or a switch is drawn: reading it lists every workspace's windows.
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
            Some(switch) => {
                let mut involved = switch.stack.clone();
                involved.push(shown);
                switch.restacked(in_order(&order, &involved), shown, now)
            }
            None if order.contains(&was_shown) => {
                let stack = in_order(&order, &[was_shown, shown]);
                let place = |workspace| index_of(&stack, workspace).map(|i| i * slide.height);
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

    /// This switch with its stack replaced by `stack`, sliding toward `shown` from `now`.
    ///
    /// Of the workspaces in both stacks, the one nearest the view keeps its place on screen: the
    /// position moves by as many heights as that workspace moves in the stack, its velocity kept.
    /// So a workspace inserted outside the two on screen moves neither, and one gone moves
    /// neither of those that stay. A workspace inserted between the two on screen puts one of
    /// them out of view: the nearer stays. `None` when `shown` is not in `stack`, or when no
    /// workspace is in both.
    fn restacked(
        &self,
        stack: Vec<WorkspaceId>,
        shown: WorkspaceId,
        now: Duration,
    ) -> Option<Switch> {
        let target = index_of(&stack, shown)? * self.height;
        let position = self.position.value(now);
        let distance = |index: usize| (index as f64 * self.height - position).abs();
        let (old_index, new_index) = self
            .stack
            .iter()
            .enumerate()
            .filter_map(|(old, &workspace)| {
                Some((old, stack.iter().position(|&w| w == workspace)?))
            })
            .min_by(|a, b| distance(a.0).total_cmp(&distance(b.0)))?;
        let shift = (new_index as f64 - old_index as f64) * self.height;
        let position = if shift == 0.0 && self.position.target() == target {
            self.position
        } else {
            self.position.shifted(shift).retarget(now, target)
        };
        Some(Switch {
            stack,
            position,
            height: self.height,
        })
    }

    /// The workspaces involved, in the output's order.
    pub fn stack(&self) -> &[WorkspaceId] {
        &self.stack
    }

    /// How far down the workspace at `index` of the stack is drawn at `at`, in logical units.
    pub fn slide(&self, index: usize, at: Duration) -> f64 {
        index as f64 * self.height - self.position.value(at)
    }

    /// When the slide arrives.
    pub fn end(&self) -> Duration {
        self.position.end()
    }
}

/// The workspaces of `involved` that `order` lists, in its order.
fn in_order(order: &[WorkspaceId], involved: &[WorkspaceId]) -> Vec<WorkspaceId> {
    order
        .iter()
        .copied()
        .filter(|workspace| involved.contains(workspace))
        .collect()
}

/// The index of `workspace` in `stack`, as a number of heights.
fn index_of(stack: &[WorkspaceId], workspace: WorkspaceId) -> Option<f64> {
    stack
        .iter()
        .position(|&w| w == workspace)
        .map(|index| index as f64)
}
