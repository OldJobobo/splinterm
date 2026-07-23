use splinterm_terminal::{
    ActiveScreen, CellExtent, ImageAlphaMode, ImageErasePolicy, ImageError, ImageRetention,
    ImageSourceFormat, NewImageContent, NewImagePlacement, PixelRect, SnapshotRequest, Terminal,
    TerminalConfig, TerminalDamage,
};

fn content(pixels: &[u8], retention: ImageRetention) -> NewImageContent<'_> {
    NewImageContent {
        width: 1,
        height: 1,
        source_format: ImageSourceFormat::Sixel,
        alpha_mode: ImageAlphaMode::Opaque,
        pixels,
        retention,
    }
}

fn placement(content_id: splinterm_terminal::ImageContentId, row_id: u64) -> NewImagePlacement {
    NewImagePlacement {
        content_id,
        row_id,
        column: 0,
        source: PixelRect {
            x: 0,
            y: 0,
            width: 1,
            height: 1,
        },
        destination: CellExtent {
            columns: 1,
            rows: 1,
        },
        x_offset: 0,
        y_offset: 0,
        z_index: 0,
        application_image_id: None,
        application_placement_id: None,
        erase_policy: ImageErasePolicy::TextOverwrite,
    }
}

fn insert_at_cursor(
    terminal: &mut Terminal,
    retention: ImageRetention,
    erase_policy: ImageErasePolicy,
) -> (
    splinterm_terminal::ImageContentId,
    splinterm_terminal::ImagePlacementId,
) {
    let content_id = terminal
        .insert_image_content(content(&[0, 0, 255, 255], retention))
        .unwrap();
    let mut input = placement(content_id, terminal.cursor_row_id());
    input.erase_policy = erase_policy;
    let placement_id = terminal.insert_image_placement(input).unwrap();
    (content_id, placement_id)
}

#[test]
fn terminal_snapshots_reference_images_without_pixel_bodies() {
    let mut terminal = Terminal::new(4, 2, TerminalConfig::default());
    let base = terminal.revision();
    let content_id = terminal
        .insert_image_content(content(&[0, 0, 255, 255], ImageRetention::ExplicitDelete))
        .unwrap();
    let placement_id = terminal
        .insert_image_placement(placement(content_id, terminal.cursor_row_id()))
        .unwrap();

    let snapshot = terminal.snapshot(SnapshotRequest::default());
    let metadata = snapshot.image_contents().collect::<Vec<_>>();
    let placements = snapshot.image_placements().collect::<Vec<_>>();
    assert_eq!(metadata.len(), 1);
    assert_eq!(metadata[0].id, content_id);
    assert_eq!(metadata[0].byte_charge, 4);
    assert_eq!(placements.len(), 1);
    assert_eq!(placements[0].id, placement_id);
    assert_eq!(
        terminal.image_content(content_id).unwrap().pixels(),
        &[0, 0, 255, 255]
    );

    let updates = terminal.updates_since(base).unwrap();
    assert_eq!(updates.updates().count(), 2);
    assert!(
        updates
            .updates()
            .all(|update| update.damage().any(|damage| {
                *damage
                    == TerminalDamage::Images {
                        screen: ActiveScreen::Normal,
                    }
            }))
    );
}

#[test]
fn bounded_image_update_history_requires_resnapshot_after_a_gap() {
    let mut terminal = Terminal::new(
        4,
        2,
        TerminalConfig {
            update_history_limit: 1,
            ..TerminalConfig::default()
        },
    );
    let base = terminal.revision();
    insert_at_cursor(
        &mut terminal,
        ImageRetention::ExplicitDelete,
        ImageErasePolicy::ExplicitDelete,
    );
    let gap = terminal.updates_since(base).unwrap_err();
    assert_eq!(gap.requested(), base);
    assert_eq!(gap.current(), terminal.revision());
}

#[test]
fn invalid_anchor_is_rejected_without_revision_or_accounting_change() {
    let mut terminal = Terminal::new(4, 2, TerminalConfig::default());
    let content_id = terminal
        .insert_image_content(content(&[0, 255, 0, 255], ImageRetention::ExplicitDelete))
        .unwrap();
    let revision = terminal.revision();
    let metrics = terminal.image_metrics();
    assert_eq!(
        terminal.insert_image_placement(placement(content_id, u64::MAX)),
        Err(ImageError::InvalidAnchor)
    );
    assert_eq!(terminal.revision(), revision);
    assert_eq!(terminal.image_metrics(), metrics);
}

#[test]
fn normal_and_fresh_alternate_image_catalogs_are_isolated() {
    let mut terminal = Terminal::new(4, 2, TerminalConfig::default());
    let normal = terminal
        .insert_image_content(content(&[255, 0, 0, 255], ImageRetention::ExplicitDelete))
        .unwrap();
    terminal
        .insert_image_placement(placement(normal, terminal.cursor_row_id()))
        .unwrap();

    terminal.advance(b"\x1b[?1049h");
    assert_eq!(terminal.active_screen(), ActiveScreen::Alternate);
    assert_eq!(
        terminal
            .snapshot(SnapshotRequest::default())
            .image_contents()
            .count(),
        0
    );
    let alternate = terminal
        .insert_image_content(content(&[0, 0, 255, 255], ImageRetention::ExplicitDelete))
        .unwrap();
    terminal
        .insert_image_placement(placement(alternate, terminal.cursor_row_id()))
        .unwrap();

    terminal.advance(b"\x1b[?1049l");
    let normal_snapshot = terminal.snapshot(SnapshotRequest::default());
    assert_eq!(normal_snapshot.image_contents().count(), 1);
    assert_eq!(normal_snapshot.image_placements().count(), 1);

    terminal.advance(b"\x1b[?1049h");
    let fresh_alternate = terminal.snapshot(SnapshotRequest::default());
    assert_eq!(fresh_alternate.image_contents().count(), 0);
    assert_eq!(fresh_alternate.image_placements().count(), 0);
}

#[test]
fn text_overwrite_and_erase_remove_only_sixel_policy_placements() {
    let mut terminal = Terminal::new(4, 2, TerminalConfig::default());
    insert_at_cursor(
        &mut terminal,
        ImageRetention::WhilePlaced,
        ImageErasePolicy::TextOverwrite,
    );
    terminal.advance(b"x");
    let overwritten = terminal.snapshot(SnapshotRequest::default());
    assert_eq!(overwritten.image_contents().count(), 0);
    assert_eq!(overwritten.image_placements().count(), 0);

    let mut terminal = Terminal::new(4, 2, TerminalConfig::default());
    insert_at_cursor(
        &mut terminal,
        ImageRetention::ExplicitDelete,
        ImageErasePolicy::ExplicitDelete,
    );
    terminal.advance(b"x\x1b[2K");
    let explicit = terminal.snapshot(SnapshotRequest::default());
    assert_eq!(explicit.image_contents().count(), 1);
    assert_eq!(explicit.image_placements().count(), 1);

    let mut terminal = Terminal::new(4, 2, TerminalConfig::default());
    insert_at_cursor(
        &mut terminal,
        ImageRetention::WhilePlaced,
        ImageErasePolicy::TextOverwrite,
    );
    terminal.advance(b"\x1b[2K");
    assert_eq!(
        terminal
            .snapshot(SnapshotRequest::default())
            .image_placements()
            .count(),
        0
    );
}

#[test]
fn character_edits_overwrite_sixel_policy_placements() {
    for command in [
        b"\x1b[@".as_slice(),
        b"\x1b[P".as_slice(),
        b"\x1b[X".as_slice(),
    ] {
        let mut terminal = Terminal::new(4, 2, TerminalConfig::default());
        insert_at_cursor(
            &mut terminal,
            ImageRetention::WhilePlaced,
            ImageErasePolicy::TextOverwrite,
        );
        terminal.advance(command);
        assert_eq!(terminal.image_metrics().placement_count, 0);
        assert_eq!(terminal.image_metrics().content_count, 0);
    }
}

#[test]
fn line_insertion_and_deletion_follow_stable_row_anchors() {
    let mut terminal = Terminal::new(4, 3, TerminalConfig::default());
    terminal.advance(b"\x1b[2;1H");
    insert_at_cursor(
        &mut terminal,
        ImageRetention::WhilePlaced,
        ImageErasePolicy::TextOverwrite,
    );
    let anchor = terminal
        .snapshot(SnapshotRequest::default())
        .image_placements()
        .next()
        .unwrap()
        .row_id;
    terminal.advance(b"\x1b[L");
    let inserted = terminal.snapshot(SnapshotRequest::default());
    assert_eq!(inserted.image_placements().next().unwrap().row_id, anchor);
    assert!(inserted.visible_rows().any(|row| row.id() == Some(anchor)));
    terminal.advance(b"\x1b[M");
    let deleted = terminal.snapshot(SnapshotRequest::default());
    assert_eq!(deleted.image_placements().next().unwrap().row_id, anchor);
    assert!(deleted.visible_rows().any(|row| row.id() == Some(anchor)));
}

#[test]
fn history_trim_and_clear_prune_stale_anchors_and_reclaim_sixel_content() {
    let mut terminal = Terminal::new(
        2,
        2,
        TerminalConfig {
            scrollback_lines: 2,
            ..TerminalConfig::default()
        },
    );
    insert_at_cursor(
        &mut terminal,
        ImageRetention::WhilePlaced,
        ImageErasePolicy::TextOverwrite,
    );
    let original_anchor = terminal
        .snapshot(SnapshotRequest::default())
        .image_placements()
        .next()
        .unwrap()
        .row_id;
    let base = terminal.revision();
    terminal.advance(b"\r\n\r\n");
    let scrolled = terminal.snapshot(SnapshotRequest::default());
    let placement = scrolled.image_placements().next().unwrap();
    assert_ne!(placement.row_id, original_anchor);
    assert!(
        terminal
            .updates_since(base)
            .unwrap()
            .updates()
            .any(|update| {
                update.damage().any(|damage| {
                    *damage
                        == TerminalDamage::Images {
                            screen: ActiveScreen::Normal,
                        }
                })
            })
    );
    terminal.advance(b"\x1b[3J");
    assert_eq!(
        terminal
            .snapshot(SnapshotRequest::default())
            .image_placements()
            .count(),
        0
    );
    assert_eq!(terminal.image_metrics().content_count, 0);

    insert_at_cursor(
        &mut terminal,
        ImageRetention::WhilePlaced,
        ImageErasePolicy::TextOverwrite,
    );
    for _ in 0..10 {
        terminal.advance(b"\r\n");
    }
    assert_eq!(terminal.image_metrics().placement_count, 0);
    assert_eq!(terminal.image_metrics().content_count, 0);
}

#[test]
fn resize_and_reset_apply_explicit_image_lifecycle_rules() {
    let mut terminal = Terminal::new(4, 2, TerminalConfig::default());
    insert_at_cursor(
        &mut terminal,
        ImageRetention::ExplicitDelete,
        ImageErasePolicy::ExplicitDelete,
    );
    terminal.resize(3, 2);
    let resized = terminal.snapshot(SnapshotRequest::default());
    assert_eq!(resized.image_placements().count(), 0);
    assert_eq!(resized.image_contents().count(), 1);

    terminal.advance(b"\x1bc");
    let reset = terminal.snapshot(SnapshotRequest::default());
    assert_eq!(reset.image_placements().count(), 0);
    assert_eq!(reset.image_contents().count(), 0);
}
