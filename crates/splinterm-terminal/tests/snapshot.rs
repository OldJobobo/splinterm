use splinterm_terminal::{
    ActiveScreen, CellSnapshotContent, SnapshotRequest, Terminal, TerminalConfig, TerminalDamage,
    TerminalEvent, TerminalRevision,
};

fn terminal(columns: usize, rows: usize) -> Terminal {
    Terminal::new(columns, rows, TerminalConfig::default())
}

fn row_text(row: splinterm_terminal::RowSnapshot<'_>) -> String {
    row.cells()
        .map(|cell| match cell.content() {
            CellSnapshotContent::Empty | CellSnapshotContent::Spacer { .. } => ' ',
            CellSnapshotContent::Scalar(character) => character,
            CellSnapshotContent::Composed(chars) => chars[0],
        })
        .collect()
}

#[test]
fn snapshot_resolves_composed_content_and_semantic_attributes() {
    let mut terminal = terminal(8, 2);
    terminal.advance("\x1b[1;31me\u{301}".as_bytes());

    let snapshot = terminal.snapshot(SnapshotRequest::default());
    assert_eq!(snapshot.revision(), terminal.revision());
    assert_eq!(snapshot.dimensions().columns, 8);
    assert_eq!(snapshot.active_screen(), ActiveScreen::Normal);
    let first = snapshot.visible_rows().next().unwrap();
    let cell = first.cells().next().unwrap();
    assert_eq!(
        cell.content(),
        CellSnapshotContent::Composed(&['e', '\u{301}'])
    );
    assert!(cell.attributes().bold);
    assert_eq!(cell.attributes().foreground.value(), 1);
}

#[test]
fn bounded_scrollback_returns_newest_rows_in_chronological_order() {
    let mut terminal = terminal(4, 2);
    terminal.advance(b"a\r\nb\r\nc\r\nd");

    let snapshot = terminal.snapshot(SnapshotRequest {
        max_scrollback_rows: 2,
    });
    let rows = snapshot.scrollback_rows().map(row_text).collect::<Vec<_>>();
    assert_eq!(rows, vec!["a   ", "b   "]);
    assert_eq!(snapshot.scrollback().available_rows, 2);
    assert_eq!(snapshot.scrollback().returned_rows, 2);

    let bounded = terminal.snapshot(SnapshotRequest {
        max_scrollback_rows: 1,
    });
    assert_eq!(
        bounded.scrollback_rows().map(row_text).collect::<Vec<_>>(),
        vec!["b   "]
    );
    assert_eq!(bounded.scrollback().omitted_oldest_rows, 1);
}

#[test]
fn alternate_snapshot_excludes_retained_normal_scrollback() {
    let mut terminal = terminal(4, 2);
    terminal.advance(b"a\r\nb\r\nc\x1b[?1049hALT");
    let snapshot = terminal.snapshot(SnapshotRequest {
        max_scrollback_rows: 20,
    });
    assert_eq!(snapshot.active_screen(), ActiveScreen::Alternate);
    assert_eq!(snapshot.scrollback().available_rows, 0);
    assert_eq!(snapshot.scrollback_rows().count(), 0);
}

#[test]
fn revisions_are_action_based_and_ignore_incomplete_input() {
    let mut terminal = terminal(8, 2);
    assert_eq!(terminal.revision(), TerminalRevision::default());
    terminal.advance(b"");
    terminal.advance(b"\x1b");
    assert_eq!(terminal.revision(), TerminalRevision::default());

    terminal.advance(b"\x18AB");
    assert_eq!(terminal.revision().value(), 2);
    terminal.advance(b"\x1b[2D");
    assert_eq!(terminal.revision().value(), 3);
    terminal.advance(b"\x1b[1m");
    assert_eq!(terminal.revision().value(), 3);
}

#[test]
fn deferred_wrap_changes_revision_even_when_cursor_coordinate_is_unchanged() {
    let mut terminal = terminal(3, 1);
    terminal.advance(b"abc");
    let base = terminal.revision();
    assert!(terminal.grid().cursor().deferred_wrap());

    terminal.advance(b"\x1b[C");
    assert_eq!(terminal.revision().value(), base.value() + 1);
    assert!(!terminal.grid().cursor().deferred_wrap());
    let update = terminal.updates_since(base).unwrap();
    assert!(
        update
            .updates()
            .next()
            .unwrap()
            .damage()
            .any(|damage| matches!(damage, TerminalDamage::Cursor { .. }))
    );
}

#[test]
fn idempotent_erase_and_empty_scrollback_clear_do_not_revise() {
    let mut terminal = terminal(4, 2);
    terminal.advance(b"\x1b[K\x1b[3J");
    assert_eq!(terminal.revision(), TerminalRevision::default());
}

#[test]
fn erase_scrollback_emits_scrollback_damage() {
    let mut terminal = terminal(3, 2);
    terminal.advance(b"a\r\nb\r\nc");
    let base = terminal.revision();
    terminal.advance(b"\x1b[3J");
    let updates = terminal.updates_since(base).unwrap();
    assert!(
        updates
            .updates()
            .next()
            .unwrap()
            .damage()
            .any(|damage| *damage == TerminalDamage::Scrollback)
    );
}

#[test]
fn updates_report_rows_cursor_scroll_and_full_snapshot_damage() {
    let mut terminal = terminal(3, 2);
    terminal.advance(b"A");
    let first = terminal.updates_since(TerminalRevision::default()).unwrap();
    let damage = first
        .updates()
        .next()
        .unwrap()
        .damage()
        .cloned()
        .collect::<Vec<_>>();
    assert!(
        damage
            .iter()
            .any(|item| matches!(item, TerminalDamage::Rows { start: 0, end: 1 }))
    );
    assert!(
        damage
            .iter()
            .any(|item| matches!(item, TerminalDamage::Cursor { .. }))
    );

    let base = terminal.revision();
    terminal.advance(b"bcdefg");
    let updates = terminal.updates_since(base).unwrap();
    assert!(
        updates
            .updates()
            .flat_map(splinterm_terminal::TerminalUpdate::damage)
            .any(|item| { matches!(item, TerminalDamage::Scroll { .. }) })
    );

    let base = terminal.revision();
    terminal.resize(5, 3);
    let resize = terminal.updates_since(base).unwrap();
    assert!(
        resize
            .updates()
            .next()
            .unwrap()
            .damage()
            .any(|item| *item == TerminalDamage::FullSnapshot)
    );
}

#[test]
fn bounded_history_forces_resnapshot_on_gaps_and_future_bases() {
    let config = TerminalConfig {
        update_history_limit: 2,
        ..TerminalConfig::default()
    };
    let mut terminal = Terminal::new(8, 2, config);
    terminal.advance(b"ABCD");
    assert_eq!(terminal.revision().value(), 4);

    let gap = terminal
        .updates_since(TerminalRevision::default())
        .unwrap_err();
    assert_eq!(gap.oldest_available().value(), 2);
    assert_eq!(gap.current().value(), 4);

    let retained = terminal.updates_since(TerminalRevision::new(2)).unwrap();
    assert_eq!(retained.updates().count(), 2);
    assert!(terminal.updates_since(TerminalRevision::new(9)).is_err());
    assert_eq!(
        terminal
            .updates_since(terminal.revision())
            .unwrap()
            .updates()
            .count(),
        0
    );
}

#[test]
fn event_overflow_is_explicit_and_snapshot_does_not_drain_effects() {
    let config = TerminalConfig {
        event_limit: 2,
        ..TerminalConfig::default()
    };
    let mut terminal = Terminal::new(4, 1, config);
    terminal.advance(b"\x07\x07\x07");
    {
        let snapshot = terminal.snapshot(SnapshotRequest::default());
        assert_eq!(snapshot.revision().value(), 3);
    }
    assert_eq!(
        terminal.drain_events().collect::<Vec<_>>(),
        vec![TerminalEvent::Bell, TerminalEvent::EventQueueOverflow]
    );
}

#[test]
fn reset_preserves_monotonic_revision_and_chunking_updates_match() {
    let input = b"ab\x1b[2D\x1b]2;title\x07\x1bcZ";
    let mut whole = terminal(8, 2);
    whole.advance(input);

    let mut bytewise = terminal(8, 2);
    for byte in input {
        bytewise.advance(std::slice::from_ref(byte));
    }

    assert_eq!(whole.revision(), bytewise.revision());
    assert_eq!(
        whole.updates_since(TerminalRevision::default()).unwrap(),
        bytewise.updates_since(TerminalRevision::default()).unwrap()
    );
    assert!(whole.revision().value() > 1);
}
