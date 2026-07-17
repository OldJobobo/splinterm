//! Borrowed semantic terminal snapshots.
//!
//! Snapshots are in-process read models. They deliberately exclude renderer
//! bookkeeping, wire encodings, and collection ownership choices.

use crate::{
    ActiveScreen, Attributes, Cell, CellContent, Color, ComposedTable, Coordinate, Cursor, Row,
    ScrollRegion, TerminalModes, TerminalRevision,
};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct SnapshotRequest {
    pub max_scrollback_rows: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Dimensions {
    pub columns: usize,
    pub rows: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CursorSnapshot {
    pub cursor: Cursor,
    pub viewport_position: Option<Coordinate>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ScrollbackSnapshot {
    pub available_rows: usize,
    pub returned_rows: usize,
    pub omitted_oldest_rows: usize,
}

#[allow(
    clippy::struct_excessive_bools,
    reason = "snapshot fields directly represent independent SGR rendition flags"
)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CellAttributesSnapshot {
    pub bold: bool,
    pub dim: bool,
    pub italic: bool,
    pub underline: bool,
    pub strikethrough: bool,
    pub blink: bool,
    pub conceal: bool,
    pub reverse: bool,
    pub foreground: Color,
    pub background: Color,
}

impl From<Attributes> for CellAttributesSnapshot {
    fn from(attributes: Attributes) -> Self {
        Self {
            bold: attributes.bold(),
            dim: attributes.dim(),
            italic: attributes.italic(),
            underline: attributes.underline(),
            strikethrough: attributes.strikethrough(),
            blink: attributes.blink(),
            conceal: attributes.conceal(),
            reverse: attributes.reverse(),
            foreground: attributes.foreground(),
            background: attributes.background(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CellSnapshotContent<'a> {
    Empty,
    Scalar(char),
    Composed(&'a [char]),
    Spacer { remaining: u32 },
}

#[derive(Clone, Copy, Debug)]
pub struct CellSnapshot<'a> {
    cell: &'a Cell,
    composed: &'a ComposedTable,
}

impl<'a> CellSnapshot<'a> {
    #[must_use]
    pub fn content(self) -> CellSnapshotContent<'a> {
        match self.cell.content() {
            CellContent::Empty => CellSnapshotContent::Empty,
            CellContent::Scalar(character) => CellSnapshotContent::Scalar(character),
            CellContent::Composed(key) => self
                .composed
                .chars(key)
                .map_or(CellSnapshotContent::Empty, CellSnapshotContent::Composed),
            CellContent::Spacer(remaining) => CellSnapshotContent::Spacer { remaining },
        }
    }

    #[must_use]
    pub fn attributes(self) -> CellAttributesSnapshot {
        self.cell.attributes().into()
    }
}

#[derive(Clone, Copy, Debug)]
pub struct RowSnapshot<'a> {
    row: &'a Row,
    composed: &'a ComposedTable,
}

impl<'a> RowSnapshot<'a> {
    pub(crate) const fn new(row: &'a Row, composed: &'a ComposedTable) -> Self {
        Self { row, composed }
    }

    #[must_use]
    pub const fn linebreak(self) -> bool {
        self.row.has_linebreak()
    }

    #[must_use]
    pub fn cells(self) -> impl ExactSizeIterator<Item = CellSnapshot<'a>> {
        self.row.cells().iter().map(|cell| CellSnapshot {
            cell,
            composed: self.composed,
        })
    }
}

#[derive(Debug)]
pub struct TerminalSnapshot<'a> {
    revision: TerminalRevision,
    dimensions: Dimensions,
    active_screen: ActiveScreen,
    cursor: CursorSnapshot,
    modes: TerminalModes,
    scroll_region: ScrollRegion,
    view_follows_live: bool,
    title: &'a str,
    palette: &'a [u32; 256],
    default_colors: &'a [u32; 3],
    visible_rows: Vec<RowSnapshot<'a>>,
    scrollback_rows: Vec<RowSnapshot<'a>>,
    scrollback: ScrollbackSnapshot,
}

impl<'a> TerminalSnapshot<'a> {
    #[allow(
        clippy::too_many_arguments,
        reason = "snapshot construction mirrors its semantic fields"
    )]
    pub(crate) fn new(
        revision: TerminalRevision,
        dimensions: Dimensions,
        active_screen: ActiveScreen,
        cursor: CursorSnapshot,
        modes: TerminalModes,
        scroll_region: ScrollRegion,
        view_follows_live: bool,
        title: &'a str,
        palette: &'a [u32; 256],
        default_colors: &'a [u32; 3],
        visible_rows: Vec<RowSnapshot<'a>>,
        scrollback_rows: Vec<RowSnapshot<'a>>,
        scrollback: ScrollbackSnapshot,
    ) -> Self {
        Self {
            revision,
            dimensions,
            active_screen,
            cursor,
            modes,
            scroll_region,
            view_follows_live,
            title,
            palette,
            default_colors,
            visible_rows,
            scrollback_rows,
            scrollback,
        }
    }

    #[must_use]
    pub const fn revision(&self) -> TerminalRevision {
        self.revision
    }
    #[must_use]
    pub const fn dimensions(&self) -> Dimensions {
        self.dimensions
    }
    #[must_use]
    pub const fn active_screen(&self) -> ActiveScreen {
        self.active_screen
    }
    #[must_use]
    pub const fn cursor(&self) -> CursorSnapshot {
        self.cursor
    }
    #[must_use]
    pub const fn modes(&self) -> TerminalModes {
        self.modes
    }
    #[must_use]
    pub const fn scroll_region(&self) -> ScrollRegion {
        self.scroll_region
    }
    #[must_use]
    pub const fn view_follows_live(&self) -> bool {
        self.view_follows_live
    }
    #[must_use]
    pub const fn title(&self) -> &'a str {
        self.title
    }
    #[must_use]
    pub const fn palette(&self) -> &'a [u32; 256] {
        self.palette
    }
    #[must_use]
    pub const fn default_colors(&self) -> &'a [u32; 3] {
        self.default_colors
    }
    #[must_use]
    pub fn visible_rows(&self) -> impl ExactSizeIterator<Item = RowSnapshot<'a>> + '_ {
        self.visible_rows.iter().copied()
    }
    #[must_use]
    pub fn scrollback_rows(&self) -> impl ExactSizeIterator<Item = RowSnapshot<'a>> + '_ {
        self.scrollback_rows.iter().copied()
    }
    #[must_use]
    pub const fn scrollback(&self) -> ScrollbackSnapshot {
        self.scrollback
    }
}
