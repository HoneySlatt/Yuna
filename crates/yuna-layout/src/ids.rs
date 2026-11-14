//! The ids the model gives what it creates: workspaces and scrolling columns.

use crate::{ColumnId, WorkspaceId};

/// Hands out workspace and column ids, each kind from 1 and never twice, so that an id once gone
/// never names something else ([ADR-0052](../../../docs/adr/0052-motion-in-a-pure-crate.md)). One
/// belongs to a [`crate::Layout`] and serves all its seats.
#[derive(Debug, Clone)]
pub(crate) struct Ids {
    /// The id the next workspace gets.
    next_workspace: u64,
    /// The id the next column gets.
    next_column: u64,
}

impl Default for Ids {
    fn default() -> Ids {
        Ids {
            next_workspace: 1,
            next_column: 1,
        }
    }
}

impl Ids {
    /// A new workspace's id. A counter of 64 bits never runs out: at a billion workspaces a
    /// second, it would take 584 years.
    pub fn new_workspace(&mut self) -> WorkspaceId {
        let id = WorkspaceId(self.next_workspace);
        self.next_workspace += 1;
        id
    }

    /// A new column's id; it never runs out either.
    pub fn new_column(&mut self) -> ColumnId {
        let id = ColumnId(self.next_column);
        self.next_column += 1;
        id
    }

    /// Whether `id` was handed out already.
    pub fn gave_workspace(&self, id: WorkspaceId) -> bool {
        (1..self.next_workspace).contains(&id.0)
    }

    /// Whether `id` was handed out already.
    pub fn gave_column(&self, id: ColumnId) -> bool {
        (1..self.next_column).contains(&id.0)
    }
}
