//! Dwindle: each new window splits the space of the focused one in two, Hyprland's layout
//! ([ADR-0019](../../../docs/adr/0019-the-layouts-behaviours.md)).
//!
//! The windows are the leaves of a binary tree; each inner node splits its rectangle in two at a
//! ratio, side by side when the rectangle is wider than tall, one above the other otherwise. The
//! direction is chosen from the shape at every arrangement, so a workspace moved to an output of
//! another shape splits to suit it. The tree is as deep as it has windows at most, so it is
//! never walked by recursion ([`Tree`]).
//!
//! Each window is asked its slot's size, whatever its minimum, as Hyprland does
//! (`misc:size_limits_tiled` false), and is drawn at its committed size from the slot's top-left
//! corner, clipped to the slot so that it never covers its neighbours
//! ([ADR-0026](../../../docs/adr/0026-real-clients-in-the-layout.md)). A window whose maximum
//! is smaller than the slot it would split opens floating; `Layout::open_window` decides it, from
//! [`Dwindle::opening_slot`].

use crate::area::Area;
use crate::geometry::{Rect, nearest_in_direction};
use crate::workspace::Arranged;
use crate::{Direction, Layer, WindowId};

/// The distance from `point` to the nearest point of `rect`; zero inside it.
fn distance_to_rect(point: crate::geometry::Point, rect: &Rect) -> f64 {
    let dx = (rect.x - point.x).max(point.x - rect.right()).max(0.0);
    let dy = (rect.y - point.y).max(point.y - rect.bottom()).max(0.0);
    dx.hypot(dy)
}

/// The index of a node in its [`Tree`]'s vector.
type NodeIndex = usize;

/// A node of the tree.
#[derive(Debug, Clone)]
enum Node {
    Leaf(WindowId),
    Split {
        /// The first child's share of the space, gap aside; strictly between 0 and 1.
        ratio: f64,
        /// Left or top.
        first: NodeIndex,
        /// Right or bottom.
        second: NodeIndex,
    },
}

/// A node and the split that holds it; none for the root.
#[derive(Debug, Clone)]
struct Slot {
    node: Node,
    parent: Option<NodeIndex>,
}

/// A binary tree whose nodes live in one vector and name each other by index. It is as deep as
/// it has windows, so it is walked with loops and an explicit stack, never by recursion: its depth
/// is bounded by memory alone, and dropping it frees one vector.
#[derive(Debug, Clone, Default)]
struct Tree {
    /// Every node; a removed node leaves `None` until a new one takes its place.
    slots: Vec<Option<Slot>>,
    /// The indices of the `None` slots.
    free: Vec<NodeIndex>,
    root: Option<NodeIndex>,
}

impl Tree {
    /// A chain of the windows in order: the first window, then a split of the second and the
    /// rest, and so on, as if each split the one before. Built from the end, in linear time.
    fn from_windows(windows: &[WindowId]) -> Tree {
        let mut tree = Tree::default();
        let mut rest = windows.iter().rev();
        if let Some(&last) = rest.next() {
            let mut chain = tree.insert(Node::Leaf(last), None);
            for &window in rest {
                let leaf = tree.insert(Node::Leaf(window), None);
                chain = tree.join(leaf, chain);
            }
            tree.root = Some(chain);
        }
        tree
    }

    fn is_empty(&self) -> bool {
        self.root.is_none()
    }

    fn slot(&self, index: NodeIndex) -> &Slot {
        self.slots[index]
            .as_ref()
            .expect("a tree links only to live nodes")
    }

    fn slot_mut(&mut self, index: NodeIndex) -> &mut Slot {
        self.slots[index]
            .as_mut()
            .expect("a tree links only to live nodes")
    }

    fn insert(&mut self, node: Node, parent: Option<NodeIndex>) -> NodeIndex {
        let slot = Some(Slot { node, parent });
        match self.free.pop() {
            Some(index) => {
                self.slots[index] = slot;
                index
            }
            None => {
                self.slots.push(slot);
                self.slots.len() - 1
            }
        }
    }

    fn remove(&mut self, index: NodeIndex) {
        self.slots[index] = None;
        self.free.push(index);
    }

    /// A new split of `first` and `second`, at half; they must have no parent yet.
    fn join(&mut self, first: NodeIndex, second: NodeIndex) -> NodeIndex {
        let split = self.insert(
            Node::Split {
                ratio: 0.5,
                first,
                second,
            },
            None,
        );
        self.slot_mut(first).parent = Some(split);
        self.slot_mut(second).parent = Some(split);
        split
    }

    /// The leaf that holds `window`, by a scan of the vector.
    fn leaf(&self, window: WindowId) -> Option<NodeIndex> {
        self.slots
            .iter()
            .position(|slot| matches!(slot, Some(Slot { node: Node::Leaf(w), .. }) if *w == window))
    }

    fn contains(&self, window: WindowId) -> bool {
        self.leaf(window).is_some()
    }

    /// The windows, left or top first.
    fn windows(&self) -> Vec<WindowId> {
        let mut out = Vec::new();
        let mut stack: Vec<NodeIndex> = self.root.into_iter().collect();
        while let Some(index) = stack.pop() {
            match self.slot(index).node {
                Node::Leaf(window) => out.push(window),
                Node::Split { first, second, .. } => {
                    stack.push(second);
                    stack.push(first);
                }
            }
        }
        out
    }

    /// The window of the first (`first` true) or last leaf under `index`.
    fn edge_leaf(&self, mut index: NodeIndex, first_side: bool) -> WindowId {
        loop {
            match self.slot(index).node {
                Node::Leaf(window) => return window,
                Node::Split { first, second, .. } => {
                    index = if first_side { first } else { second };
                }
            }
        }
    }

    /// Adds `window` as the root, the tree being empty.
    fn set_only_leaf(&mut self, window: WindowId) {
        let leaf = self.insert(Node::Leaf(window), None);
        self.root = Some(leaf);
    }

    /// Replaces the leaf of `target` by a split of it and `new`, `new` second. Returns whether
    /// the leaf was found.
    fn split_leaf(&mut self, target: WindowId, new: WindowId) -> bool {
        let Some(leaf) = self.leaf(target) else {
            return false;
        };
        // The leaf's slot becomes the split, so its parent's link stays right.
        let first = self.insert(Node::Leaf(target), Some(leaf));
        let second = self.insert(Node::Leaf(new), Some(leaf));
        self.slot_mut(leaf).node = Node::Split {
            ratio: 0.5,
            first,
            second,
        };
        true
    }

    /// Removes the leaf of `window`: its sibling takes their parent's place. Returns whether the
    /// leaf was found.
    fn remove_leaf(&mut self, window: WindowId) -> bool {
        let Some(leaf) = self.leaf(window) else {
            return false;
        };
        let Some(parent) = self.slot(leaf).parent else {
            self.root = None;
            self.remove(leaf);
            return true;
        };
        let Node::Split { first, second, .. } = self.slot(parent).node else {
            unreachable!("a parent is a split");
        };
        let sibling = if first == leaf { second } else { first };
        let grandparent = self.slot(parent).parent;
        self.slot_mut(sibling).parent = grandparent;
        match grandparent {
            None => self.root = Some(sibling),
            Some(grandparent) => {
                if let Node::Split { first, second, .. } = &mut self.slot_mut(grandparent).node {
                    if *first == parent {
                        *first = sibling;
                    } else {
                        *second = sibling;
                    }
                }
            }
        }
        self.remove(leaf);
        self.remove(parent);
        true
    }

    /// The leaf of `window`'s sibling subtree nearest to it: the sibling's last leaf when the
    /// window is second, its first leaf when the window is first.
    fn nearest_sibling_leaf(&self, window: WindowId) -> Option<WindowId> {
        let leaf = self.leaf(window)?;
        let parent = self.slot(leaf).parent?;
        let Node::Split { first, second, .. } = self.slot(parent).node else {
            return None;
        };
        Some(if first == leaf {
            self.edge_leaf(second, true)
        } else {
            self.edge_leaf(first, false)
        })
    }

    /// Exchanges the leaves of `a` and `b`.
    fn swap_leaves(&mut self, a: WindowId, b: WindowId) {
        let (Some(leaf_a), Some(leaf_b)) = (self.leaf(a), self.leaf(b)) else {
            return;
        };
        self.slot_mut(leaf_a).node = Node::Leaf(b);
        self.slot_mut(leaf_b).node = Node::Leaf(a);
    }

    /// Every window with its rectangle inside `rect`, left or top first.
    fn arrange(&self, rect: Rect, area: &Area, out: &mut Vec<(WindowId, Rect)>) {
        let mut stack: Vec<(NodeIndex, Rect)> =
            self.root.into_iter().map(|root| (root, rect)).collect();
        while let Some((index, rect)) = stack.pop() {
            match self.slot(index).node {
                Node::Leaf(window) => out.push((window, area.scale.snap_rect(rect))),
                Node::Split {
                    ratio,
                    first,
                    second,
                } => {
                    let (a, b) = split_rect(rect, ratio, area);
                    stack.push((second, b));
                    stack.push((first, a));
                }
            }
        }
    }

    /// Checks the links: every split's children point back to it, the root has no parent, every
    /// live node is reachable from the root, and every ratio is strictly between 0 and 1.
    fn check(&self) -> Result<(), String> {
        let live = self.slots.iter().filter(|slot| slot.is_some()).count();
        if live + self.free.len() != self.slots.len() {
            return Err("the dwindle tree's free list and its slots disagree".to_owned());
        }
        let mut reached = 0;
        let mut stack: Vec<(NodeIndex, Option<NodeIndex>)> =
            self.root.into_iter().map(|root| (root, None)).collect();
        while let Some((index, parent)) = stack.pop() {
            reached += 1;
            if reached > live {
                return Err("the dwindle tree has a cycle".to_owned());
            }
            let Some(slot) = self.slots.get(index).and_then(Option::as_ref) else {
                return Err(format!("the dwindle tree links to a dead node {index}"));
            };
            if slot.parent != parent {
                return Err(format!("node {index} does not point back to its parent"));
            }
            if let Node::Split {
                ratio,
                first,
                second,
            } = slot.node
            {
                if !(ratio > 0.0 && ratio < 1.0) {
                    return Err(format!("a split has ratio {ratio}"));
                }
                stack.push((first, Some(index)));
                stack.push((second, Some(index)));
            }
        }
        if reached != live {
            return Err("the dwindle tree holds nodes its root does not reach".to_owned());
        }
        Ok(())
    }

    /// The tree's shape, in prefix order: a window for a leaf, `None` for a split.
    #[cfg(test)]
    fn shape(&self) -> Vec<Option<WindowId>> {
        let mut out = Vec::new();
        let mut stack: Vec<NodeIndex> = self.root.into_iter().collect();
        while let Some(index) = stack.pop() {
            match self.slot(index).node {
                Node::Leaf(window) => out.push(Some(window)),
                Node::Split { first, second, .. } => {
                    out.push(None);
                    stack.push(second);
                    stack.push(first);
                }
            }
        }
        out
    }
}

/// Splits `rect` in two at `ratio`, side by side when it is wider than tall, one above the other
/// otherwise. Each part stays inside `rect`, however deep the tree: below a gap's size, parts
/// shrink to nothing rather than spill out.
fn split_rect(rect: Rect, ratio: f64, area: &Area) -> (Rect, Rect) {
    let gap = area.gap;
    if rect.w > rect.h {
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
    }
}

/// The dwindle tree of one workspace, with its focus and its fullscreen or maximised window.
#[derive(Debug, Clone, Default)]
pub(crate) struct Dwindle {
    tree: Tree,
    focused: Option<WindowId>,
    /// The window that covers the output, the tree left as it is beneath. One at most.
    fullscreen: Option<WindowId>,
    /// The window that covers the working area, over the others (full width in dwindle). One at
    /// most.
    maximized: Option<WindowId>,
}

impl Dwindle {
    /// Builds a tree from windows in order, each splitting the one before, the window at `focus`
    /// focused (the last one without it), in time linear in the windows.
    pub fn from_windows(windows: &[WindowId], focus: Option<WindowId>) -> Dwindle {
        let tree = Tree::from_windows(windows);
        let focused = focus
            .filter(|f| windows.contains(f))
            .or(windows.last().copied());
        Dwindle {
            tree,
            focused,
            ..Dwindle::default()
        }
    }

    /// Whether the tree holds no window.
    pub fn is_empty(&self) -> bool {
        self.tree.is_empty()
    }

    /// Every window, left or top first.
    pub fn windows(&self) -> Vec<WindowId> {
        self.tree.windows()
    }

    /// Whether `window` is a leaf.
    pub fn contains(&self, window: WindowId) -> bool {
        self.tree.contains(window)
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
        match (self.tree.root, self.focused) {
            (None, _) => self.tree.set_only_leaf(window),
            (Some(_), Some(focused)) => {
                self.tree.split_leaf(focused, window);
            }
            (Some(root), None) => {
                let last = self.tree.edge_leaf(root, false);
                self.tree.split_leaf(last, window);
            }
        }
        if self.fullscreen.is_none() {
            // The new window takes the focus: a maximised one would hide it, so it stops being.
            self.maximized = None;
            self.focused = Some(window);
        }
    }

    /// Removes `window`; returns whether it was here. Its sibling takes their parent's space. If
    /// it was focused, focus goes to the window whose rectangle, once the tree is laid out again,
    /// is nearest the closed window's centre (Hyprland's `input:focus_on_close` 0); without an
    /// area to measure, to the nearest window of the sibling's part.
    pub fn remove_window(&mut self, window: WindowId, area: Option<&Area>) -> bool {
        if !self.tree.contains(window) {
            return false;
        }
        let mut before = Vec::new();
        if let Some(area) = area {
            self.arrange_tree(area, &mut before);
        }
        let closed_centre = before
            .iter()
            .find(|(w, _)| *w == window)
            .map(|(_, r)| r.center());
        let sibling_leaf = self.tree.nearest_sibling_leaf(window);
        self.tree.remove_leaf(window);
        if self.focused == Some(window) {
            // Hyprland lays the tree out again first, then takes the window nearest the closed
            // one's centre: the one now covering it, if any.
            let nearest = match (closed_centre, area) {
                (Some(centre), Some(area)) => {
                    let mut after = Vec::new();
                    self.arrange_tree(area, &mut after);
                    after
                        .iter()
                        .map(|(w, r)| (*w, distance_to_rect(centre, r)))
                        .min_by(|a, b| a.1.total_cmp(&b.1))
                        .map(|(w, _)| w)
                }
                _ => None,
            };
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
        self.tree.swap_leaves(focused, next);
    }

    /// Makes `window` fullscreen or not; returns whether it is here. One window at most covers
    /// the others, fullscreen or maximised: another one going fullscreen ends that. The
    /// fullscreen window takes focus.
    pub fn set_fullscreen(&mut self, window: WindowId, on: bool) -> bool {
        if !self.contains(window) {
            return false;
        }
        if on {
            // One covering window at a time; a window keeps its own maximised state under its
            // fullscreen and finds it again after, as Hyprland ("restore client maximized state").
            if self.maximized != Some(window) {
                self.maximized = None;
            }
            self.fullscreen = Some(window);
            self.focused = Some(window);
        } else if self.fullscreen == Some(window) {
            self.fullscreen = None;
        }
        true
    }

    /// Maximises `window` over the working area, or not; returns whether it is here. One window
    /// at most covers the others, fullscreen or maximised: another one maximised ends that. The
    /// maximised window takes focus.
    pub fn set_maximized(&mut self, window: WindowId, on: bool) -> bool {
        if !self.contains(window) {
            return false;
        }
        if on {
            if self.fullscreen != Some(window) {
                self.fullscreen = None;
            }
            self.maximized = Some(window);
            self.focused = Some(window);
        } else if self.maximized == Some(window) {
            self.maximized = None;
        }
        true
    }

    /// The tree's slots alone, before fullscreen and maximise.
    fn arrange_tree(&self, area: &Area, out: &mut Vec<(WindowId, Rect)>) {
        self.tree.arrange(area.working.shrunk(area.gap), area, out);
    }

    /// The slot a new window would split: the focused window's, or the last window's when none
    /// is focused, as [`Dwindle::add_window`] chooses; none when the tree is empty.
    pub fn opening_slot(&self, area: &Area) -> Option<Rect> {
        let root = self.tree.root?;
        let target = self
            .focused
            .unwrap_or_else(|| self.tree.edge_leaf(root, false));
        let mut slots = Vec::new();
        self.arrange_tree(area, &mut slots);
        slots
            .into_iter()
            .find(|&(window, _)| window == target)
            .map(|(_, slot)| slot)
    }

    /// Every window's slot, as [`Dwindle::arrange`] draws it: the tree's, the maximised window's
    /// over the working area, the fullscreen window's over the output.
    fn slots(&self, area: &Area, with_hidden: bool) -> Vec<(WindowId, Rect)> {
        let mut slots = Vec::new();
        if let Some(fullscreen) = self.fullscreen {
            if with_hidden {
                self.arrange_tree(area, &mut slots);
                slots.retain(|&(window, _)| window != fullscreen);
            }
            slots.push((fullscreen, area.output));
            return slots;
        }
        self.arrange_tree(area, &mut slots);
        if let Some(maximized) = self.maximized
            && let Some(index) = slots.iter().position(|&(w, _)| w == maximized)
        {
            slots.remove(index);
            let rect = area.scale.snap_rect(area.working.shrunk(area.gap));
            slots.push((maximized, rect));
        }
        slots
    }

    /// Every window with its rectangle and the size it is asked, in drawing order: the tree, then
    /// the maximised window over it. With a fullscreen window, only that window, centred on the
    /// output; `with_hidden` adds the tree's windows beneath it, first.
    pub fn arrange(&self, area: &Area, out: &mut Vec<Arranged>, with_hidden: bool) {
        for (window, slot) in self.slots(area, with_hidden) {
            let fullscreen = self.fullscreen == Some(window);
            let (rect, configure) = if fullscreen {
                area.place_fullscreen(window, slot)
            } else {
                area.place_in_slot(window, slot)
            };
            out.push(Arranged {
                window,
                rect,
                configure,
                layer: Layer::Tiles,
                column: None,
                fullscreen,
            });
        }
    }

    /// Checks the tree's own rules; with `area`, that each window drawn lies inside its slot, and
    /// has an area whenever its slot has one.
    pub fn check(&self, area: Option<&Area>) -> Result<(), String> {
        if let Some(area) = area {
            let mut placed = Vec::new();
            self.arrange(area, &mut placed, true);
            for (p, (window, slot)) in placed.iter().zip(self.slots(area, true)) {
                if p.window != window || !p.rect.is_inside(&slot) {
                    return Err(format!("{:?} is drawn outside its slot {slot:?}", p.window));
                }
                if slot.has_area() && !p.rect.has_area() {
                    return Err(format!("{:?} is drawn empty in slot {slot:?}", p.window));
                }
            }
        }
        let windows = self.windows();
        self.tree.check()?;
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
        if let (Some(f), Some(m)) = (self.fullscreen, self.maximized)
            && f != m
        {
            return Err("dwindle has a fullscreen and a maximised window at once".into());
        }
        for cover in [self.fullscreen, self.maximized].into_iter().flatten() {
            if self.focused != Some(cover) {
                return Err(format!(
                    "dwindle's covering window {cover:?} has not the focus"
                ));
            }
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
        assert_eq!(at_once.tree.shape(), one_by_one.tree.shape());
        assert_eq!(at_once.focused, one_by_one.focused);
    }

    /// Each new window splits the focused one, so a tree is as deep as it has windows. Nothing
    /// walks it by recursion: a tree of 100,000 windows is built, arranged, checked, changed,
    /// cloned and dropped without overflowing the stack.
    #[test]
    fn a_tree_as_deep_as_100_000_windows_does_not_overflow_the_stack() {
        let count = 100_000;
        let windows: Vec<WindowId> = (1..=count).map(WindowId).collect();
        let mut tree = Dwindle::from_windows(&windows, None);
        let records = crate::layout::Windows::default();
        let area = Area::new(
            Rect::new(0.0, 0.0, 2560.0, 1440.0),
            crate::geometry::Scale::new(1.5).unwrap(),
            16.0,
            &records,
        );
        let mut placed = Vec::new();
        tree.arrange(&area, &mut placed, false);
        assert_eq!(placed.len(), windows.len());
        assert_eq!(tree.windows(), windows);
        tree.check(Some(&area)).unwrap();
        tree.add_window(WindowId(count + 1));
        assert!(tree.contains(WindowId(count + 1)));
        assert!(tree.remove_window(WindowId(count + 1), Some(&area)));
        // Past a dozen windows, sizes are zero: focus goes to the nearest by Hyprland's rule.
        assert!(tree.focused_window().is_some_and(|w| tree.contains(w)));
        tree.move_focused(Direction::Left, &area);
        tree.check(Some(&area)).unwrap();
        let copy = tree.clone();
        drop(tree);
        drop(copy);
    }
}
