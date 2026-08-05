//! Pure terminal snapshot, semantic-update, and bounded-history reduction.

use anyhow::Result;
use splinterm_protocol::{
    CellAttributes, ColorSource, HistoryTransition, ScrollDirection, TerminalCell, TerminalRow,
    TerminalScroll, TerminalSnapshot, TerminalUpdate, UnderlineStyle,
};

pub(super) const MAX_CACHED_HISTORY_ROWS: usize = 4096;
pub(super) const MAX_CACHED_HISTORY_BYTES: usize = 16 * 1024 * 1024;

pub(super) fn snapshot_is_newer(
    current: &TerminalSnapshot,
    candidate: &TerminalSnapshot,
) -> Result<bool> {
    if candidate.splint_id != current.splint_id || candidate.incarnation != current.incarnation {
        anyhow::bail!(
            "live snapshot identity changed from {:?}/{} to {:?}/{}",
            current.splint_id,
            current.incarnation,
            candidate.splint_id,
            candidate.incarnation
        );
    }
    Ok(candidate.revision > current.revision)
}

pub(super) fn snapshot_replaces(
    current: &TerminalSnapshot,
    candidate: &TerminalSnapshot,
    authoritative: bool,
) -> Result<bool> {
    Ok(snapshot_is_newer(current, candidate)?
        || (authoritative && candidate.revision == current.revision))
}

#[cfg(test)]
pub(super) fn coalesce_snapshots(
    current: Option<&TerminalSnapshot>,
    pending: impl IntoIterator<Item = TerminalSnapshot>,
) -> Result<Option<TerminalSnapshot>> {
    let mut latest = None;
    for candidate in pending {
        let baseline = latest.as_ref().or(current);
        if match baseline {
            Some(snapshot) => snapshot_is_newer(snapshot, &candidate)?,
            None => true,
        } {
            latest = Some(candidate);
        }
    }
    Ok(latest)
}

pub(super) fn blank_row(columns: usize) -> TerminalRow {
    TerminalRow {
        row_id: None,
        linebreak: false,
        cells: vec![
            TerminalCell {
                content: String::new(),
                spacer_remaining: None,
                attributes: CellAttributes {
                    bold: false,
                    dim: false,
                    italic: false,
                    underline: UnderlineStyle::None,
                    underline_color_source: ColorSource::Default,
                    underline_color: 0,
                    strikethrough: false,
                    blink: false,
                    conceal: false,
                    reverse: false,
                    foreground_source: ColorSource::Default,
                    foreground: 0,
                    background_source: ColorSource::Default,
                    background: 0,
                },
            };
            columns
        ],
    }
}

fn terminal_row_cache_bytes(row: &TerminalRow) -> usize {
    row.cells.iter().fold(32_usize, |total, cell| {
        total.saturating_add(64).saturating_add(cell.content.len())
    })
}

pub(super) fn history_cache_bytes(rows: &[TerminalRow]) -> usize {
    rows.iter().map(terminal_row_cache_bytes).sum()
}

pub(super) fn omitted_rows_before_cache(
    oldest_available_row_id: Option<u64>,
    rows: &[TerminalRow],
    available_rows: usize,
) -> usize {
    oldest_available_row_id
        .zip(rows.first().and_then(|row| row.row_id))
        .and_then(|(oldest, first)| first.checked_sub(oldest))
        .and_then(|omitted| usize::try_from(omitted).ok())
        .map_or_else(
            || available_rows.saturating_sub(rows.len()),
            |omitted| omitted.min(available_rows),
        )
}

fn bound_history_cache(rows: &mut Vec<TerminalRow>, keep_oldest: bool) {
    while rows.len() > MAX_CACHED_HISTORY_ROWS
        || history_cache_bytes(rows) > MAX_CACHED_HISTORY_BYTES
    {
        if rows.is_empty() {
            break;
        }
        if keep_oldest {
            rows.pop();
        } else {
            rows.remove(0);
        }
    }
}

pub(super) fn bound_history_page_with_pins(
    mut rows: Vec<TerminalRow>,
    pinned_selection_rows: Option<[u64; 2]>,
    visible_rows: &[TerminalRow],
) -> Option<Vec<TerminalRow>> {
    bound_history_cache(&mut rows, true);
    pinned_selection_rows
        .is_none_or(|pins| {
            pins.into_iter().all(|row_id| {
                rows.iter().any(|row| row.row_id == Some(row_id))
                    || visible_rows.iter().any(|row| row.row_id == Some(row_id))
            })
        })
        .then_some(rows)
}

pub(super) fn changed_terminal_patch_rows(
    update: &TerminalUpdate,
    current: &TerminalSnapshot,
) -> Vec<usize> {
    if update
        .columns
        .is_some_and(|columns| columns != current.columns)
        || update.row_count.is_some_and(|rows| rows != current.rows)
    {
        return update.rows.iter().map(|patch| patch.index).collect();
    }
    let mut projected_rows = current.visible_rows.clone();
    for scroll in &update.scrolls {
        if scroll.start_row >= scroll.end_row
            || scroll.rows == 0
            || scroll.end_row > projected_rows.len()
            || scroll.rows > scroll.end_row.saturating_sub(scroll.start_row)
        {
            return update.rows.iter().map(|patch| patch.index).collect();
        }
        apply_terminal_scroll(&mut projected_rows, current.columns, *scroll);
    }
    let row_identity_is_visual = current
        .images
        .as_ref()
        .is_some_and(|images| !images.placements.is_empty());
    update
        .rows
        .iter()
        .filter(|patch| {
            projected_rows.get(patch.index).is_none_or(|row| {
                row.cells != patch.row.cells
                    || (row_identity_is_visual && row.row_id != patch.row.row_id)
            })
        })
        .map(|patch| patch.index)
        .collect()
}

pub(super) fn terminal_update_changes_visible_content(update: &TerminalUpdate) -> bool {
    !update.rows.is_empty()
        || !update.scrolls.is_empty()
        || update.columns.is_some()
        || update.row_count.is_some()
        || update.palette.is_some()
        || update.default_colors.is_some()
        || update.active_screen.is_some()
        || update.scrollback.is_some()
        || update.images.is_some()
}

pub(super) fn terminal_update_full_frame_reasons(
    update: &TerminalUpdate,
    current: &TerminalSnapshot,
) -> u64 {
    u64::from(
        update
            .columns
            .is_some_and(|columns| columns != current.columns),
    ) | (u64::from(update.row_count.is_some_and(|rows| rows != current.rows)) << 1)
        | (u64::from(
            update
                .palette
                .as_ref()
                .is_some_and(|palette| palette.get(16..) != current.palette.get(16..)),
        ) << 2)
        | (u64::from(
            update
                .active_screen
                .is_some_and(|active_screen| active_screen != current.active_screen),
        ) << 4)
        | (u64::from(
            update
                .images
                .as_ref()
                .is_some_and(|images| current.images.as_ref() != Some(images)),
        ) << 5)
        | (u64::from(
            current
                .images
                .as_ref()
                .is_some_and(|images| !images.placements.is_empty())
                && !update.scrolls.is_empty(),
        ) << 6)
}

#[cfg(test)]
pub(super) fn terminal_update_requires_full_frame(
    update: &TerminalUpdate,
    current: &TerminalSnapshot,
) -> bool {
    terminal_update_full_frame_reasons(update, current) != 0
}

pub(super) fn apply_scrollback_update(
    snapshot: &mut TerminalSnapshot,
    scrollback: splinterm_protocol::TerminalScrollbackUpdate,
) -> Result<()> {
    match scrollback.transition {
        HistoryTransition::Append { .. }
            if scrollback.history_generation != snapshot.history_generation =>
        {
            anyhow::bail!("history append changed generation");
        }
        HistoryTransition::Clear | HistoryTransition::Reflow
            if scrollback.history_generation <= snapshot.history_generation =>
        {
            anyhow::bail!("history reset did not change generation");
        }
        _ => {}
    }
    let preserve_cached = scrollback.history_generation == snapshot.history_generation
        && matches!(
            scrollback.transition,
            HistoryTransition::Append { .. } | HistoryTransition::Replace
        );
    let first_returned = scrollback.rows.first().and_then(|row| row.row_id);
    let oldest_available = scrollback.oldest_available_row_id;
    let mut rows = if preserve_cached {
        snapshot
            .scrollback_rows
            .iter()
            .filter(|row| {
                row.row_id
                    .zip(oldest_available)
                    .is_some_and(|(id, oldest)| id >= oldest)
                    && row
                        .row_id
                        .zip(first_returned)
                        .is_some_and(|(id, first)| id < first)
            })
            .cloned()
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };
    rows.extend(scrollback.rows);
    bound_history_cache(&mut rows, false);
    snapshot.history_generation = scrollback.history_generation;
    snapshot.oldest_available_scrollback_row_id = scrollback.oldest_available_row_id;
    snapshot.newest_available_scrollback_row_id = scrollback.newest_available_row_id;
    snapshot.scrollback_rows = rows;
    snapshot.available_scrollback_rows = scrollback.available_rows;
    snapshot.omitted_oldest_scrollback_rows = snapshot
        .available_scrollback_rows
        .saturating_sub(snapshot.scrollback_rows.len());
    Ok(())
}

fn apply_terminal_scroll(visible_rows: &mut [TerminalRow], columns: usize, scroll: TerminalScroll) {
    let region = &mut visible_rows[scroll.start_row..scroll.end_row];
    match scroll.direction {
        ScrollDirection::Forward => {
            region.rotate_left(scroll.rows);
            let exposed_start = region.len() - scroll.rows;
            for row in &mut region[exposed_start..] {
                *row = blank_row(columns);
            }
        }
        ScrollDirection::Reverse => {
            region.rotate_right(scroll.rows);
            for row in &mut region[..scroll.rows] {
                *row = blank_row(columns);
            }
        }
    }
}

pub(super) fn apply_terminal_update(
    snapshot: &mut TerminalSnapshot,
    update: TerminalUpdate,
) -> Result<()> {
    update
        .validate_against(
            snapshot.revision,
            snapshot.history_generation,
            snapshot.columns,
            snapshot.rows,
        )
        .map_err(|error| anyhow::anyhow!(error.message))?;
    if let Some(columns) = update.columns {
        if columns == 0 || columns > usize::from(splinterm_protocol::MAX_COLUMNS) {
            anyhow::bail!("terminal update columns exceed protocol limits");
        }
        snapshot.columns = columns;
    }
    if let Some(rows) = update.row_count {
        if rows == 0 || rows > usize::from(splinterm_protocol::MAX_ROWS) {
            anyhow::bail!("terminal update rows exceed protocol limits");
        }
        snapshot.rows = rows;
        snapshot
            .visible_rows
            .resize_with(rows, || blank_row(snapshot.columns));
        snapshot.visible_rows.truncate(rows);
    }
    for scroll in update.scrolls {
        apply_terminal_scroll(&mut snapshot.visible_rows, snapshot.columns, scroll);
    }
    for patch in update.rows {
        if patch.index >= snapshot.rows || patch.row.cells.len() > snapshot.columns {
            anyhow::bail!("terminal row patch exceeds current dimensions");
        }
        snapshot.visible_rows[patch.index] = patch.row;
    }
    if let Some(cursor) = update.cursor {
        snapshot.cursor_column = cursor.column;
        snapshot.cursor_row = cursor.row;
        snapshot.cursor_deferred_wrap = cursor.deferred_wrap;
    }
    if let Some(title) = update.title {
        snapshot.title = title;
    }
    if let Some(modes) = update.input_modes {
        snapshot.input_modes = modes;
    }
    if let Some(screen) = update.active_screen {
        snapshot.active_screen = screen;
    }
    if let Some(palette) = update.palette {
        if palette.len() != 256 {
            anyhow::bail!("terminal update palette must have 256 entries");
        }
        snapshot.palette = palette;
    }
    if let Some(colors) = update.default_colors {
        snapshot.default_colors = colors;
    }
    if let Some(scrollback) = update.scrollback {
        apply_scrollback_update(snapshot, scrollback)?;
    }
    if let Some(images) = update.images {
        snapshot.images = Some(images);
    }
    snapshot.revision = update.revision;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn history_row(id: u64, content_bytes: usize) -> TerminalRow {
        let mut row = blank_row(1);
        row.row_id = Some(id);
        row.cells[0].content = "x".repeat(content_bytes);
        row
    }

    #[test]
    fn history_cache_enforces_row_budget_from_either_edge() {
        let mut newest = (1..=u64::try_from(MAX_CACHED_HISTORY_ROWS + 4).unwrap())
            .map(|id| history_row(id, 0))
            .collect::<Vec<_>>();
        bound_history_cache(&mut newest, false);
        assert_eq!(newest.len(), MAX_CACHED_HISTORY_ROWS);
        assert_eq!(newest.first().and_then(|row| row.row_id), Some(5));

        let mut oldest = (1..=u64::try_from(MAX_CACHED_HISTORY_ROWS + 4).unwrap())
            .map(|id| history_row(id, 0))
            .collect::<Vec<_>>();
        bound_history_cache(&mut oldest, true);
        assert_eq!(oldest.len(), MAX_CACHED_HISTORY_ROWS);
        assert_eq!(oldest.last().and_then(|row| row.row_id), Some(4096));
    }

    #[test]
    fn history_cache_enforces_byte_budget_and_preserves_order() {
        let mut rows = (1..=20)
            .map(|id| history_row(id, 1024 * 1024))
            .collect::<Vec<_>>();
        bound_history_cache(&mut rows, false);
        assert!(history_cache_bytes(&rows) <= MAX_CACHED_HISTORY_BYTES);
        assert!(rows.windows(2).all(|pair| pair[0].row_id < pair[1].row_id));
        assert_eq!(rows.last().and_then(|row| row.row_id), Some(20));
    }

    #[test]
    fn omitted_history_tracks_the_stable_cache_window_position() {
        let rows = |first| {
            (first..first + 100)
                .map(|id| history_row(id, 0))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            omitted_rows_before_cache(Some(100), &rows(1_000), 1_000),
            900
        );
        assert_eq!(omitted_rows_before_cache(Some(100), &rows(500), 1_000), 400);
        assert_eq!(omitted_rows_before_cache(Some(100), &rows(100), 1_000), 0);
        assert_eq!(omitted_rows_before_cache(None, &rows(1_000), 1_000), 900);
    }
}
