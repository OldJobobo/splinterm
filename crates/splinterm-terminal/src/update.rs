//! Semantic terminal revisions, damage, bounded replay, and resnapshot gaps.

use crate::{Cursor, ScrollDirection, ScrollRegion, TerminalEvent};

/// Monotonic semantic terminal revision.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct TerminalRevision(u64);

impl TerminalRevision {
    #[must_use]
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    #[must_use]
    pub const fn value(self) -> u64 {
        self.0
    }

    pub(crate) fn next(self) -> Self {
        Self(self.0.checked_add(1).expect("terminal revision exhausted"))
    }
}

/// Renderer-independent semantic damage associated with one revision.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TerminalDamage {
    FullSnapshot,
    Rows {
        start: usize,
        end: usize,
    },
    Scroll {
        direction: ScrollDirection,
        region: ScrollRegion,
        rows: usize,
    },
    Cursor {
        old: Cursor,
        new: Cursor,
    },
    Modes,
    Viewport,
    Dimensions,
    Scrollback,
    Title,
    Palette {
        index: Option<u16>,
    },
}

/// One committed semantic transition.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TerminalUpdate {
    revision: TerminalRevision,
    damage: Vec<TerminalDamage>,
    events: Vec<TerminalEvent>,
}

impl TerminalUpdate {
    pub(crate) fn new(
        revision: TerminalRevision,
        damage: Vec<TerminalDamage>,
        events: Vec<TerminalEvent>,
    ) -> Self {
        Self {
            revision,
            damage,
            events,
        }
    }

    #[must_use]
    pub const fn revision(&self) -> TerminalRevision {
        self.revision
    }

    #[must_use]
    pub fn damage(&self) -> impl ExactSizeIterator<Item = &TerminalDamage> {
        self.damage.iter()
    }

    #[must_use]
    pub fn events(&self) -> impl ExactSizeIterator<Item = &TerminalEvent> {
        self.events.iter()
    }
}

/// Contiguous updates after a requested base revision.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UpdateBatch {
    base: TerminalRevision,
    current: TerminalRevision,
    updates: Vec<TerminalUpdate>,
}

impl UpdateBatch {
    pub(crate) fn new(
        base: TerminalRevision,
        current: TerminalRevision,
        updates: Vec<TerminalUpdate>,
    ) -> Self {
        Self {
            base,
            current,
            updates,
        }
    }

    #[must_use]
    pub const fn base(&self) -> TerminalRevision {
        self.base
    }

    #[must_use]
    pub const fn current(&self) -> TerminalRevision {
        self.current
    }

    #[must_use]
    pub fn updates(&self) -> impl ExactSizeIterator<Item = &TerminalUpdate> {
        self.updates.iter()
    }
}

/// The requested update base is unavailable; the caller must take a snapshot.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ResnapshotRequired {
    requested: TerminalRevision,
    oldest_available: TerminalRevision,
    current: TerminalRevision,
}

impl ResnapshotRequired {
    pub(crate) const fn new(
        requested: TerminalRevision,
        oldest_available: TerminalRevision,
        current: TerminalRevision,
    ) -> Self {
        Self {
            requested,
            oldest_available,
            current,
        }
    }

    #[must_use]
    pub const fn requested(self) -> TerminalRevision {
        self.requested
    }

    #[must_use]
    pub const fn oldest_available(self) -> TerminalRevision {
        self.oldest_available
    }

    #[must_use]
    pub const fn current(self) -> TerminalRevision {
        self.current
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct ChangeSet {
    pub(crate) damage: Vec<TerminalDamage>,
    pub(crate) events: Vec<TerminalEvent>,
}

impl ChangeSet {
    pub(crate) fn row(&mut self, row: usize) {
        self.rows(row, row + 1);
    }

    pub(crate) fn rows(&mut self, start: usize, end: usize) {
        if start >= end
            || self
                .damage
                .iter()
                .any(|item| matches!(item, TerminalDamage::FullSnapshot))
        {
            return;
        }
        if let Some(TerminalDamage::Rows {
            start: existing_start,
            end: existing_end,
        }) = self
            .damage
            .iter_mut()
            .find(|item| matches!(item, TerminalDamage::Rows { .. }))
        {
            if start <= *existing_end && end >= *existing_start {
                *existing_start = (*existing_start).min(start);
                *existing_end = (*existing_end).max(end);
                return;
            }
        }
        self.damage.push(TerminalDamage::Rows { start, end });
    }

    pub(crate) fn full(&mut self) {
        self.damage.clear();
        self.damage.push(TerminalDamage::FullSnapshot);
    }

    pub(crate) fn push(&mut self, damage: TerminalDamage) {
        if !self.damage.contains(&damage)
            && !self
                .damage
                .iter()
                .any(|item| matches!(item, TerminalDamage::FullSnapshot))
        {
            self.damage.push(damage);
        }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.damage.is_empty() && self.events.is_empty()
    }
}
