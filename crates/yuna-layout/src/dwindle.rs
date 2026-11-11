//! Dwindle: each new window splits the space of the focused one in two, Hyprland's layout
//! ([ADR-0019](../../../docs/adr/0019-the-layouts-behaviours.md)).
//!
//! The windows are the leaves of a binary tree; each inner node splits its rectangle in two at a
//! ratio, side by side when the rectangle is wider than tall, one above the other otherwise. The
//! direction is chosen from the shape at every arrangement, so a workspace moved to an output of
//! another shape splits to suit it. The tree is as deep as it has windows at most; the recursion
//! below is bounded by that.

use crate::area::Area;
use crate::geometry::{Rect, nearest_in_direction};
use crate::{Direction, WindowId};

/// A node of the tree.
#[derive(Debug, Clone)]
enum Node {
    Leaf(WindowId),
    Split {
        /// The first child's share of the space, gap aside; strictly between 0 and 1.
        ratio: f64,
        /// Left or top.
        first: Box<Node>,
        /// Right or bottom.
        second: Box<Node>,
    },
}

impl Node {
    fn split(first: Node, second: Node) -> Node {
        Node::Split {
            ratio: 0.5,
            first: Box::new(first),
            second: Box::new(second),
        }
    }

    /// Pushes the leaves, left or top first.
    fn collect_leaves(&self, out: &mut Vec<WindowId>) {
        match self {
            Node::Leaf(window) => out.push(*window),
            Node::Split { first, second, .. } => {
                first.collect_leaves(out);
                second.collect_leaves(out);
            }
        }
    }

    fn first_leaf(&self) -> WindowId {
        match self {
            Node::Leaf(window) => *window,
            Node::Split { first, .. } => first.first_leaf(),
        }
    }

    fn last_leaf(&self) -> WindowId {
        match self {
            Node::Leaf(window) => *window,
            Node::Split { second, .. } => second.last_leaf(),
        }
    }

    /// Replaces the leaf `target` by a split of it and `new`, `new` second. Returns whether the
    /// leaf was found.
    fn split_leaf(&mut self, target: WindowId, new: WindowId) -> bool {
        match self {
            Node::Leaf(window) if *window == target => {
                let old = std::mem::replace(self, Node::Leaf(new));
                *self = Node::split(old, Node::Leaf(new));
                true
            }
            Node::Leaf(_) => false,
            Node::Split { first, second, .. } => {
                first.split_leaf(target, new) || second.split_leaf(target, new)
            }
        }
    }

    /// The tree without the leaf `window`: the leaf's sibling takes their parent's place.
    fn without(self, window: WindowId) -> Option<Node> {
        match self {
            Node::Leaf(w) if w == window => None,
            Node::Leaf(w) => Some(Node::Leaf(w)),
            Node::Split {
                ratio,
                first,
                second,
            } => match (first.without(window), second.without(window)) {
                (Some(first), Some(second)) => Some(Node::Split {
                    ratio,
                    first: Box::new(first),
                    second: Box::new(second),
                }),
                (Some(only), None) | (None, Some(only)) => Some(only),
                (None, None) => None,
            },
        }
    }

    /// The leaf of `window`'s sibling subtree nearest to it: the sibling's last leaf when the
    /// window is second, its first leaf when the window is first.
    fn nearest_sibling_leaf(&self, window: WindowId) -> Option<WindowId> {
        let Node::Split { first, second, .. } = self else {
            return None;
        };
        if matches!(**first, Node::Leaf(w) if w == window) {
            return Some(second.first_leaf());
        }
        if matches!(**second, Node::Leaf(w) if w == window) {
            return Some(first.last_leaf());
        }
        first
            .nearest_sibling_leaf(window)
            .or_else(|| second.nearest_sibling_leaf(window))
    }

    /// Exchanges the leaves `a` and `b`.
    fn swap_leaves(&mut self, a: WindowId, b: WindowId) {
        match self {
            Node::Leaf(window) if *window == a => *window = b,
            Node::Leaf(window) if *window == b => *window = a,
            Node::Leaf(_) => {}
            Node::Split { first, second, .. } => {
                first.swap_leaves(a, b);
                second.swap_leaves(a, b);
            }
        }
    }

    fn arrange(&self, rect: Rect, area: &Area, out: &mut Vec<(WindowId, Rect)>) {
        match self {
            Node::Leaf(window) => out.push((*window, area.scale.snap_rect(rect))),
            Node::Split {
                ratio,
                first,
                second,
            } => {
                let gap = area.gap;
                // Each part stays inside its parent, however deep the tree: below a gap's
                // size, parts shrink to nothing rather than spill out.
                let (a, b) = if rect.w > rect.h {
                    let w = area.scale.snap(((rect.w - gap) * ratio).max(0.0));
                    let second_x = (rect.x + w + gap).min(rect.right());
                    (
                        Rect::new(rect.x, rect.y, w.min(rect.w), rect.h),
                        Rect::new(second_x, rect.y, rect.right() - second_x, rect.h),
                    )
                } else {
                    let h = area.scale.snap(((rect.h - gap) * ratio).max(0.0));
                    let second_y = (rect.y + h + gap).min(rect.bottom());
                    (
                        Rect::new(rect.x, rect.y, rect.w, h.min(rect.h)),
                        Rect::new(rect.x, second_y, rect.w, rect.bottom() - second_y),
                    )
                };
                first.arrange(a, area, out);
                second.arrange(b, area, out);
            }
        }
    }

    fn check(&self) -> Result<(), String> {
        match self {
            Node::Leaf(_) => Ok(()),
            Node::Split {
                ratio,
                first,
                second,
            } => {
                if !(*ratio > 0.0 && *ratio < 1.0) {
                    return Err(format!("a split has ratio {ratio}"));
                }
                first.check()?;
                second.check()
            }
        }
    }
}

/// The dwindle tree of one workspace, with its focus and its fullscreen or maximised window.
#[derive(Debug, Clone, Default)]
pub(crate) struct Dwindle {
    root: Option<Node>,
    focused: Option<WindowId>,
    /// The window that covers the output, the tree left as it is beneath. One at most.
    fullscreen: Option<WindowId>,
    /// The window that covers the working area, over the others (full width in dwindle). One at
    /// most.
    maximized: Option<WindowId>,
}

impl Dwindle {
    /// Builds a tree from windows in order, each splitting the one before, the window at `focus`
    /// focused (the last one without it). Each new window splits the previous one, so the tree is
    /// a chain: the first window, then a split of the second and the rest, and so on. Built from
    /// the end, in time linear in the windows.
    pub fn from_windows(windows: &[WindowId], focus: Option<WindowId>) -> Dwindle {
        let mut rest = windows.iter().rev();
        let root = rest.next().map(|&last| {
            rest.fold(Node::Leaf(last), |chain, &window| {
                Node::split(Node::Leaf(window), chain)
            })
        });
        let focused = focus
            .filter(|f| windows.contains(f))
            .or(windows.last().copied());
        Dwindle {
            root,
            focused,
            ..Dwindle::default()
        }
    }

    /// Whether the tree holds no window.
    pub fn is_empty(&self) -> bool {
        self.root.is_none()
    }

    /// Every window, left or top first.
    pub fn windows(&self) -> Vec<WindowId> {
        let mut out = Vec::new();
        if let Some(root) = &self.root {
            root.collect_leaves(&mut out);
        }
        out
    }

    /// Whether `window` is a leaf.
    pub fn contains(&self, window: WindowId) -> bool {
        self.windows().contains(&window)
    }

    /// The focused window.
    pub fn focused_window(&self) -> Option<WindowId> {
        self.focused
    }

    /// The fullscreen window.
    pub fn fullscreen_window(&self) -> Option<WindowId> {
        self.fullscreen
    }

    /// The maximised window.
    pub fn maximized_window(&self) -> Option<WindowId> {
        self.maximized
    }

    /// Splits the focused window's space with `window`, the new one right or below, and focuses
    /// it, ending a maximised window; unless a window is fullscreen: then the new window opens
    /// behind it and focus stays, so that nothing takes a game's screen.
    pub fn add_window(&mut self, window: WindowId) {
        match (&mut self.root, self.focused) {
            (None, _) => self.root = Some(Node::Leaf(window)),
            (Some(root), Some(focused)) => {
                root.split_leaf(focused, window);
            }
            (Some(root), None) => {
                let last = root.last_leaf();
                root.split_leaf(last, window);
            }
        }
        if self.fullscreen.is_none() {
            // The new window takes the focus: a maximised one would hide it, so it stops being.
            self.maximized = None;
            self.focused = Some(window);
        }
    }

    /// Removes `window`; returns whether it was here. Its sibling takes their parent's space. If
    /// it was focused, focus goes to the window whose centre is nearest its centre (Hyprland's
    /// default); without an area to measure, to the nearest window of the sibling's part.
    pub fn remove_window(&mut self, window: WindowId, area: Option<&Area>) -> bool {
        let Some(root) = self.root.take() else {
            return false;
        };
        let mut placed = Vec::new();
        if let Some(area) = area {
            root.arrange(area.working.shrunk(area.gap), area, &mut placed);
        }
        let closed_centre = placed
            .iter()
            .find(|(w, _)| *w == window)
            .map(|(_, r)| r.center());
        let sibling_leaf = root.nearest_sibling_leaf(window);
        let mut leaves = Vec::new();
        root.collect_leaves(&mut leaves);
        self.root = root.without(window);
        if !leaves.contains(&window) {
            return false;
        }
        if self.focused == Some(window) {
            let nearest = closed_centre.and_then(|c| {
                placed
                    .iter()
                    .filter(|(w, _)| *w != window)
                    .map(|(w, r)| (*w, (r.center().x - c.x).hypot(r.center().y - c.y)))
                    .min_by(|a, b| a.1.total_cmp(&b.1))
                    .map(|(w, _)| w)
            });
            self.focused = nearest.or(sibling_leaf);
        }
        if self.fullscreen == Some(window) {
            self.fullscreen = None;
        }
        if self.maximized == Some(window) {
            self.maximized = None;
        }
        true
    }

    /// Focuses `window`; returns whether it is here. Focusing another window than the fullscreen
    /// or maximised one ends that state, so that the focused window is never hidden.
    pub fn focus_window(&mut self, window: WindowId) -> bool {
        if !self.contains(window) {
            return false;
        }
        self.focus_ending_cover(window);
        true
    }

    fn focus_ending_cover(&mut self, window: WindowId) {
        if self.fullscreen.is_some_and(|f| f != window) {
            self.fullscreen = None;
        }
        if self.maximized.is_some_and(|m| m != window) {
            self.maximized = None;
        }
        self.focused = Some(window);
    }

    /// The window next to the focused one in `direction`, by their rectangles in the tree.
    fn neighbour(&self, direction: Direction, area: &Area) -> Option<WindowId> {
        let focused = self.focused?;
        let mut placed = Vec::new();
        self.arrange_tree(area, &mut placed);
        let from = placed.iter().find(|(w, _)| *w == focused)?.1;
        nearest_in_direction(
            from,
            direction,
            placed.iter().filter(|(w, _)| *w != focused),
        )
    }

    /// Moves focus to the neighbour in `direction`, if there is one. A fullscreen window keeps
    /// focus: its neighbours are hidden.
    pub fn focus(&mut self, direction: Direction, area: &Area) {
        if self.fullscreen.is_some() {
            return;
        }
        if let Some(next) = self.neighbour(direction, area) {
            self.focus_ending_cover(next);
        }
    }

    /// Exchanges the focused window with its neighbour in `direction`; focus follows the window.
    pub fn move_focused(&mut self, direction: Direction, area: &Area) {
        if self.fullscreen.is_some() {
            return;
        }
        let (Some(focused), Some(next)) = (self.focused, self.neighbour(direction, area)) else {
            return;
        };
        if let Some(root) = &mut self.root {
            root.swap_leaves(focused, next);
        }
    }

    /// Makes `window` fullscreen or not; returns whether it is here. One window at most is
    /// fullscreen: another one leaves it. The fullscreen window takes focus.
    pub fn set_fullscreen(&mut self, window: WindowId, on: bool) -> bool {
        if !self.contains(window) {
            return false;
        }
        if on {
            self.fullscreen = Some(window);
            self.focused = Some(window);
        } else if self.fullscreen == Some(window) {
            self.fullscreen = None;
        }
        true
    }

    /// Maximises `window` over the working area, or not; returns whether it is here. One window
    /// at most is maximised: another one leaves it. The maximised window takes focus.
    pub fn set_maximized(&mut self, window: WindowId, on: bool) -> bool {
        if !self.contains(window) {
            return false;
        }
        if on {
            self.maximized = Some(window);
            self.focused = Some(window);
        } else if self.maximized == Some(window) {
            self.maximized = None;
        }
        true
    }

    /// The tree's rectangles alone, before fullscreen and maximise.
    fn arrange_tree(&self, area: &Area, out: &mut Vec<(WindowId, Rect)>) {
        if let Some(root) = &self.root {
            root.arrange(area.working.shrunk(area.gap), area, out);
        }
    }

    /// Every window with its rectangle, in drawing order: the tree, then the maximised window
    /// over it. With a fullscreen window, only that window, over the whole output.
    pub fn arrange(&self, area: &Area, out: &mut Vec<(WindowId, Rect)>) {
        if let Some(fullscreen) = self.fullscreen {
            out.push((fullscreen, area.output));
            return;
        }
        let start = out.len();
        self.arrange_tree(area, out);
        if let Some(maximized) = self.maximized
            && let Some(index) = out[start..].iter().position(|(w, _)| *w == maximized)
        {
            out.remove(start + index);
            let rect = area.scale.snap_rect(area.working.shrunk(area.gap));
            out.push((maximized, rect));
        }
    }

    /// Checks the tree's own rules.
    pub fn check(&self) -> Result<(), String> {
        let windows = self.windows();
        if let Some(root) = &self.root {
            root.check()?;
        }
        let mut sorted = windows.clone();
        sorted.sort_unstable_by_key(|w| w.0);
        sorted.dedup();
        if sorted.len() != windows.len() {
            return Err("a window is twice in the dwindle tree".to_owned());
        }
        match self.focused {
            None if !windows.is_empty() => return Err("dwindle has windows but no focus".into()),
            Some(f) if !windows.contains(&f) => return Err(format!("dwindle focuses {f:?}")),
            _ => {}
        }
        for (what, window) in [
            ("fullscreen", self.fullscreen),
            ("maximized", self.maximized),
        ] {
            if let Some(w) = window
                && !windows.contains(&w)
            {
                return Err(format!("dwindle's {what} window {w:?} is not in the tree"));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tree_built_at_once_is_the_tree_built_window_by_window() {
        let windows: Vec<WindowId> = (1..=6).map(WindowId).collect();
        let mut one_by_one = Dwindle::default();
        for &window in &windows {
            one_by_one.add_window(window);
        }
        let at_once = Dwindle::from_windows(&windows, None);
        assert_eq!(
            format!("{:?}", at_once.root),
            format!("{:?}", one_by_one.root)
        );
        assert_eq!(at_once.focused, one_by_one.focused);
    }
}
