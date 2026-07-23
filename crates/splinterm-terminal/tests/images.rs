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
