//! Edits predicted by the client and not yet answered by the server.
//!
//! The player's edit shows at once: it is kept here, over the columns of the [`Mirror`]
//! (which hold only what the server sent), until the server answers. For each cell, a queue
//! of edits in the order they were made (breaking then placing at the same cell does not
//! flicker when the first answer comes):
//! - a `Blocks` batch that covers the cell confirms the oldest edit if it holds the same
//!   block; with another block the server is right and the whole queue goes;
//! - `EditRefused` at the cell removes the oldest edit;
//! - an edit left unanswered longer than the timeout goes.
//!
//! When an edit goes, the cell shows what the mirror holds under it: the original block, or
//! a newer one if the server changed it since.
//!
//! [`Mirror`]: crate::Mirror

use std::collections::{BTreeMap, VecDeque};
use std::time::{Duration, Instant};

use nascio_core::coords::BlockPos;

/// One predicted edit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PendingEdit {
    /// The block the player put (air: broken).
    pub block: u16,
    /// The block shown before the edit.
    pub original: u16,
    /// When it was made.
    pub at: Instant,
}

/// Edits not yet answered, by cell (positions are wrapped by [`BlockPos::new`]).
#[derive(Clone, Debug, Default)]
pub struct PendingEdits {
    cells: BTreeMap<BlockPos, VecDeque<PendingEdit>>,
}

impl PendingEdits {
    /// Adds an edit at `pos`, after those already waiting there.
    pub fn push(&mut self, pos: BlockPos, edit: PendingEdit) {
        self.cells.entry(pos).or_default().push_back(edit);
    }

    /// The block predicted at `pos` (the newest edit there), if any.
    pub fn get(&self, pos: BlockPos) -> Option<u16> {
        self.cells.get(&pos)?.back().map(|e| e.block)
    }

    /// The edits waiting at `pos`, oldest first.
    pub fn at(&self, pos: BlockPos) -> impl Iterator<Item = &PendingEdit> {
        self.cells.get(&pos).into_iter().flatten()
    }

    /// The server put `block` at `pos`: the oldest edit there is confirmed if it holds the same
    /// block, else every edit there goes (the server is right).
    pub fn confirm(&mut self, pos: BlockPos, block: u16) {
        let Some(q) = self.cells.get_mut(&pos) else {
            return;
        };
        if q.front().is_some_and(|e| e.block == block) {
            q.pop_front();
        } else {
            q.clear();
        }
        if q.is_empty() {
            self.cells.remove(&pos);
        }
    }

    /// The server refused the edit at `pos`: the oldest one there goes.
    pub fn refuse(&mut self, pos: BlockPos) {
        if let Some(q) = self.cells.get_mut(&pos) {
            q.pop_front();
            if q.is_empty() {
                self.cells.remove(&pos);
            }
        }
    }

    /// Removes the edits made `timeout` or more before `now`; returns the cells that lost one.
    pub fn expire(&mut self, now: Instant, timeout: Duration) -> Vec<BlockPos> {
        let mut gone = Vec::new();
        self.cells.retain(|&pos, q| {
            let before = q.len();
            while q
                .front()
                .is_some_and(|e| now.saturating_duration_since(e.at) >= timeout)
            {
                q.pop_front();
            }
            if q.len() != before {
                gone.push(pos);
            }
            !q.is_empty()
        });
        gone
    }

    /// Every cell with a predicted block, with that block.
    pub fn iter(&self) -> impl Iterator<Item = (BlockPos, u16)> + '_ {
        self.cells
            .iter()
            .filter_map(|(&p, q)| Some((p, q.back()?.block)))
    }

    /// Number of cells with edits waiting.
    pub fn len(&self) -> usize {
        self.cells.len()
    }

    pub fn is_empty(&self) -> bool {
        self.cells.is_empty()
    }
}
