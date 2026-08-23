use splinterm_terminal::{
    ActiveScreen, CellExtent, ImageAlphaMode, ImageErasePolicy, ImageError, ImageLimits,
    ImageRetention, ImageSourceFormat, NewImageContent, NewImagePlacement,
    NewImagePlacementOptions, PixelRect, PixelSize, SixelConfig, SnapshotRequest, Terminal,
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
        source_cell_size: Some(PixelSize {
            width: 1,
            height: 1,
        }),
        x_offset: 0,
        y_offset: 0,
        z_index: 0,
        application_image_id: None,
        application_placement_id: None,
        erase_policy: ImageErasePolicy::TextOverwrite,
    }
}

fn placement_options(erase_policy: ImageErasePolicy) -> NewImagePlacementOptions {
    NewImagePlacementOptions {
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
        source_cell_size: Some(PixelSize {
            width: 1,
            height: 1,
        }),
        x_offset: 0,
        y_offset: 0,
        z_index: 0,
        application_image_id: None,
        application_placement_id: None,
        erase_policy,
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

fn decode_fixture_hex(value: &str) -> Vec<u8> {
    assert_eq!(value.len() % 2, 0);
    value
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect()
}

fn expand_fixture_rows(expected: &serde_json::Value) -> Vec<u8> {
    expected["rows"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|row| {
            row.as_array().unwrap().iter().flat_map(|run| {
                let run = run.as_array().unwrap();
                let count = usize::try_from(run[0].as_u64().unwrap()).unwrap();
                decode_fixture_hex(run[1].as_str().unwrap()).repeat(count)
            })
        })
        .collect()
}

#[test]
fn streamed_sixel_matches_every_pinned_foot_fixture_at_every_chunk_boundary() {
    let fixtures: serde_json::Value = serde_json::from_str(include_str!(
        "../../../fixtures/terminal-images/v1/protocol-fixtures/sixel-v1.json"
    ))
    .unwrap();
    assert_eq!(
        fixtures["authority"]["commit"],
        "3c5b584b0eafa772eb4376fb6eaf6643399e190e"
    );
    let cases = fixtures["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 5);

    for case in cases {
        let id = case["id"].as_str().unwrap();
        let input = decode_fixture_hex(case["input_hex"].as_str().unwrap());
        let build = |chunks: &[&[u8]]| {
            let mut terminal = Terminal::new(4, 3, TerminalConfig::default());
            terminal.set_cell_pixel_size(1, 6);
            for chunk in chunks {
                terminal.advance(chunk);
            }
            terminal
        };
        let expected = build(&[&input]);
        assert_eq!(expected.image_metrics().content_count, 1, "{id}");
        assert_eq!(expected.image_metrics().placement_count, 1, "{id}");
        let snapshot = expected.snapshot(SnapshotRequest::default());
        let metadata = snapshot.image_contents().next().unwrap();
        let expected_width = u32::try_from(case["expected"]["width"].as_u64().unwrap()).unwrap();
        let expected_height = u32::try_from(case["expected"]["height"].as_u64().unwrap()).unwrap();
        assert_eq!(
            (metadata.width, metadata.height),
            (expected_width, expected_height),
            "{id}"
        );
        assert_eq!(
            metadata.alpha_mode,
            if case["expected"]["opaque"].as_bool().unwrap() {
                ImageAlphaMode::Opaque
            } else {
                ImageAlphaMode::Premultiplied
            },
            "{id}"
        );
        assert_eq!(
            expected.image_content(metadata.id).unwrap().pixels(),
            expand_fixture_rows(&case["expected"]),
            "{id}"
        );

        for split in 0..=input.len() {
            assert_eq!(
                build(&[&input[..split], &input[split..]]),
                expected,
                "{id} split {split}"
            );
        }
        let byte_chunks = input.iter().map(std::slice::from_ref).collect::<Vec<_>>();
        assert_eq!(build(&byte_chunks), expected, "{id} bytewise");
    }
}

#[test]
fn phase5_scaled_sixel_payload_decodes_with_live_cell_geometry() {
    let mut terminal = Terminal::new(80, 24, TerminalConfig::default());
    terminal.set_cell_pixel_size(7, 17);
    terminal.advance(b"\x1bP7;0;0q\"1;1;10;12#1;2;100;0;0#1!10~-!10~\x1b\\");

    assert_eq!(terminal.image_metrics().content_count, 1);
    assert_eq!(terminal.image_metrics().placement_count, 1);
    let snapshot = terminal.snapshot(SnapshotRequest::default());
    let metadata = snapshot
        .image_contents()
        .next()
        .expect("scaled Sixel image");
    assert_eq!((metadata.width, metadata.height), (10, 12));
    assert_eq!(
        terminal.image_content(metadata.id).unwrap().pixels().len(),
        10 * 12 * 4
    );
}

#[test]
fn sixel_configuration_uses_foot_palette_and_can_disable_graphics() {
    let mut terminal = Terminal::new(4, 2, TerminalConfig::default());
    terminal.set_cell_pixel_size(1, 6);
    terminal.advance(b"\x1bP7;0;0q\"1;1;1;6#2~\x1b\\");
    let metadata = terminal
        .snapshot(SnapshotRequest::default())
        .image_contents()
        .next()
        .unwrap();
    assert_eq!(
        terminal.image_content(metadata.id).unwrap().pixels(),
        [0x21, 0x21, 0xcc, 0xff].repeat(6)
    );

    let mut disabled = Terminal::new(
        4,
        2,
        TerminalConfig {
            sixel: SixelConfig {
                enabled: false,
                ..SixelConfig::default()
            },
            ..TerminalConfig::default()
        },
    );
    disabled.set_cell_pixel_size(1, 6);
    disabled.advance(b"\x1bP7;0;0q#1~\x1b\\X");
    assert_eq!(disabled.image_metrics().content_count, 0);
    assert_eq!(
        disabled
            .snapshot(SnapshotRequest::default())
            .visible_rows()
            .next()
            .unwrap()
            .cells()
            .next()
            .unwrap()
            .content(),
        splinterm_terminal::CellSnapshotContent::Scalar('X')
    );
}

#[test]
fn sixel_shared_palette_persists_definitions_and_private_mode_resets_them() {
    let mut terminal = Terminal::new(
        4,
        3,
        TerminalConfig {
            sixel: SixelConfig {
                private_palette: false,
                ..SixelConfig::default()
            },
            ..TerminalConfig::default()
        },
    );
    terminal.set_cell_pixel_size(1, 6);
    terminal.advance(
        b"\x1bP7;0;0q\"1;1;1;6#31;2;100;0;0#31~\x1b\\\x1b[2;1H\x1bP7;0;0q\"1;1;1;6#31~\x1b\\",
    );
    let snapshot = terminal.snapshot(SnapshotRequest::default());
    let pixels = snapshot
        .image_contents()
        .map(|content| {
            terminal
                .image_content(content.id)
                .unwrap()
                .pixels()
                .to_vec()
        })
        .collect::<Vec<_>>();
    assert_eq!(pixels.len(), 2);
    assert!(
        pixels
            .iter()
            .all(|pixels| *pixels == [0, 0, 255, 255].repeat(6))
    );

    terminal.advance(b"\x1b[?1070h\x1b[3;1H\x1bP7;0;0q\"1;1;1;6#31~\x1b\\");
    let snapshot = terminal.snapshot(SnapshotRequest::default());
    let last = snapshot.image_contents().last().unwrap();
    assert_eq!(
        terminal.image_content(last.id).unwrap().pixels(),
        [0, 0, 0, 0].repeat(6)
    );
}

#[test]
fn sixel_shared_palette_survives_cancel_and_decoder_failure() {
    let mut terminal = Terminal::new(
        4,
        3,
        TerminalConfig {
            sixel: SixelConfig {
                private_palette: false,
                ..SixelConfig::default()
            },
            ..TerminalConfig::default()
        },
    );
    terminal.set_cell_pixel_size(1, 6);
    terminal.advance(b"\x1bP7;0;0q#31;2;100;0;0#31?\x18\x1bP7;0;0q\"1;1;1;6#31~\x1b\\");
    terminal.advance(b"\x1b[2;1H\x1bP7;0;0q#32;2;0;100;0#32?/\x1b\\\x1bP7;0;0q\"1;1;1;6#32~\x1b\\");
    let snapshot = terminal.snapshot(SnapshotRequest::default());
    let pixels = snapshot
        .image_contents()
        .map(|content| {
            terminal
                .image_content(content.id)
                .unwrap()
                .pixels()
                .to_vec()
        })
        .collect::<Vec<_>>();
    assert_eq!(
        pixels,
        vec![[0, 0, 255, 255].repeat(6), [0, 255, 0, 255].repeat(6)]
    );
}

#[test]
fn sixel_modes_control_anchor_and_post_image_cursor() {
    let image = b"\x1bP7;0;0q\"1;1;2;12#1;2;100;0;0#1~~-~~\x1b\\";
    let mut terminal = Terminal::new(6, 4, TerminalConfig::default());
    terminal.set_cell_pixel_size(1, 6);
    terminal.advance(b"\x1b[2;2H\x1b[?8452h");
    let base = terminal.revision();
    terminal.advance(image);
    assert_eq!(terminal.revision().value(), base.value() + 1);
    let snapshot = terminal.snapshot(SnapshotRequest::default());
    let placement = snapshot.image_placements().next().unwrap();
    assert_eq!(placement.column, 1);
    assert_eq!(placement.destination.columns, 2);
    assert_eq!(placement.destination.rows, 2);
    assert_eq!(snapshot.cursor().cursor.position().row, 2);
    assert_eq!(snapshot.cursor().cursor.position().column, 3);

    let mut display_mode = Terminal::new(6, 4, TerminalConfig::default());
    display_mode.set_cell_pixel_size(1, 6);
    display_mode.advance(b"\x1b[3;4H\x1b[?80h");
    let cursor = display_mode
        .snapshot(SnapshotRequest::default())
        .cursor()
        .cursor;
    let base = display_mode.revision();
    display_mode.advance(image);
    assert_eq!(display_mode.revision().value(), base.value() + 1);
    let snapshot = display_mode.snapshot(SnapshotRequest::default());
    let placement = snapshot.image_placements().next().unwrap();
    assert_eq!(placement.column, 0);
    assert_eq!(
        placement.row_id,
        snapshot.visible_rows().next().unwrap().id().unwrap()
    );
    assert_eq!(snapshot.cursor().cursor, cursor);
}

#[test]
fn xtsmgraphics_queries_are_bounded_and_ordered_before_later_replies() {
    let mut terminal = Terminal::new(10, 4, TerminalConfig::default());
    terminal.set_cell_pixel_size(8, 16);
    terminal.advance(b"\x1b[?1;3;64S\x1b[?1;1S\x1b[?2;1S\x1b[?2;4S\x1b[c");
    assert_eq!(
        terminal.drain_events().collect::<Vec<_>>(),
        vec![
            splinterm_terminal::TerminalEvent::PtyWrite(b"\x1b[?1;0;64S".to_vec()),
            splinterm_terminal::TerminalEvent::PtyWrite(b"\x1b[?1;0;64S".to_vec()),
            splinterm_terminal::TerminalEvent::PtyWrite(b"\x1b[?2;0;80;64S".to_vec()),
            splinterm_terminal::TerminalEvent::PtyWrite(b"\x1b[?2;0;4096;4096S".to_vec()),
            splinterm_terminal::TerminalEvent::PtyWrite(b"\x1b[?62;22c".to_vec()),
        ]
    );
}

#[test]
fn cancelled_sixel_discards_partial_pixels_and_resynchronizes() {
    let mut terminal = Terminal::new(4, 2, TerminalConfig::default());
    terminal.set_cell_pixel_size(1, 6);
    terminal.advance(b"\x1bP7;0;0q#1;2;100;0;0#1~\x18Z");
    assert_eq!(terminal.image_metrics().content_count, 0);
    assert_eq!(terminal.image_metrics().placement_count, 0);
    assert_eq!(
        terminal
            .snapshot(SnapshotRequest::default())
            .visible_rows()
            .next()
            .unwrap()
            .cells()
            .next()
            .unwrap()
            .content(),
        splinterm_terminal::CellSnapshotContent::Scalar('Z')
    );
}

#[test]
fn transmit_and_display_is_atomic_and_commits_one_revision() {
    let mut terminal = Terminal::new(4, 2, TerminalConfig::default());
    let base = terminal.revision();
    terminal
        .insert_image_at_cursor(
            content(&[0, 0, 255, 255], ImageRetention::WhilePlaced),
            placement_options(ImageErasePolicy::TextOverwrite),
        )
        .unwrap();
    assert_eq!(terminal.revision().value(), base.value() + 1);
    assert_eq!(terminal.image_metrics().content_count, 1);
    assert_eq!(terminal.image_metrics().placement_count, 1);

    let mut rejected = Terminal::new(
        4,
        2,
        TerminalConfig {
            image_limits: ImageLimits {
                placements_per_terminal: 0,
                ..ImageLimits::default()
            },
            ..TerminalConfig::default()
        },
    );
    let revision = rejected.revision();
    let metrics = rejected.image_metrics();
    assert_eq!(
        rejected.insert_image_at_cursor(
            content(&[0, 0, 255, 255], ImageRetention::WhilePlaced),
            placement_options(ImageErasePolicy::TextOverwrite),
        ),
        Err(ImageError::PlacementCount)
    );
    assert_eq!(rejected.revision(), revision);
    assert_eq!(rejected.image_metrics(), metrics);
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
fn opaque_sixel_overlap_splits_the_older_placement_around_the_new_image() {
    let mut terminal = Terminal::new(5, 5, TerminalConfig::default());
    terminal.set_cell_pixel_size(1, 6);
    terminal.advance(
        b"\x1bP7;0;0q\"1;1;3;18#1;2;100;0;0#1~~~\x1b\\\x1b[2;2H\x1bP7;0;0q\"1;1;1;6#2;2;0;100;0#2~\x1b\\",
    );
    let snapshot = terminal.snapshot(SnapshotRequest::default());
    assert_eq!(snapshot.image_contents().count(), 2);
    assert_eq!(snapshot.image_placements().count(), 5);
    let old_id = snapshot.image_contents().next().unwrap().id;
    let old_fragments = snapshot
        .image_placements()
        .filter(|placement| placement.content_id == old_id)
        .collect::<Vec<_>>();
    assert_eq!(old_fragments.len(), 4);
    assert!(old_fragments.iter().all(|placement| {
        placement.destination.columns * placement.destination.rows == 3
            || placement.destination.columns * placement.destination.rows == 1
    }));
}

#[test]
fn same_count_sixel_crop_emits_image_damage() {
    let mut terminal = Terminal::new(4, 2, TerminalConfig::default());
    terminal.set_cell_pixel_size(1, 6);
    terminal.advance(b"\x1bP7;0;0q\"1;1;2;6#1;2;100;0;0#1~~\x1b\\");
    let base = terminal.revision();
    terminal.advance(b"X");
    assert_eq!(terminal.image_metrics().placement_count, 1);
    let updates = terminal.updates_since(base).unwrap();
    assert!(updates.updates().any(|update| {
        update.damage().any(|damage| {
            *damage
                == TerminalDamage::Images {
                    screen: ActiveScreen::Normal,
                }
        })
    }));
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
fn sixel_anchor_survives_narrow_and_wide_normal_screen_reflow() {
    let mut terminal = Terminal::new(4, 3, TerminalConfig::default());
    terminal.set_cell_pixel_size(1, 6);
    terminal.advance(b"ABCDE\x1bP7;0;0q\"1;1;1;6#1;2;100;0;0#1~\x1b\\");
    assert_eq!(terminal.image_metrics().placement_count, 1);

    terminal.resize(2, 3);
    let narrow = terminal.snapshot(SnapshotRequest::default());
    let placement = narrow.image_placements().next().unwrap();
    assert_eq!(placement.column, 1);
    assert!(
        narrow
            .visible_rows()
            .any(|row| row.id() == Some(placement.row_id))
    );

    terminal.resize(4, 3);
    let wide = terminal.snapshot(SnapshotRequest::default());
    let placement = wide.image_placements().next().unwrap();
    assert!(
        wide.visible_rows()
            .any(|row| row.id() == Some(placement.row_id))
    );
}

#[test]
fn normal_resize_preserves_sixel_on_trailing_blank_non_cursor_row() {
    let mut terminal = Terminal::new(4, 3, TerminalConfig::default());
    terminal.set_cell_pixel_size(1, 6);
    terminal.advance(b"\x1b[3;1H\x1bP7;0;0q\"1;1;1;6#1;2;100;0;0#1~\x1b\\\x1b[2;1H");
    assert_eq!(terminal.image_metrics().placement_count, 1);
    terminal.resize(2, 3);
    let snapshot = terminal.snapshot(SnapshotRequest::default());
    assert_eq!(snapshot.image_placements().count(), 1);
    let placement = snapshot.image_placements().next().unwrap();
    assert!(
        snapshot
            .visible_rows()
            .any(|row| row.id() == Some(placement.row_id))
    );
}

#[test]
fn alternate_resize_remaps_visible_sixel_and_drops_out_of_bounds_start_column() {
    let mut retained = Terminal::new(4, 3, TerminalConfig::default());
    retained.set_cell_pixel_size(1, 6);
    retained.advance(b"\x1b[?1049h\x1b[2;2H\x1bP7;0;0q\"1;1;1;6#1;2;100;0;0#1~\x1b\\");
    retained.resize(2, 3);
    let snapshot = retained.snapshot(SnapshotRequest::default());
    assert_eq!(snapshot.image_placements().count(), 1);
    let placement = snapshot.image_placements().next().unwrap();
    assert_eq!(placement.column, 1);
    assert!(
        snapshot
            .visible_rows()
            .any(|row| row.id() == Some(placement.row_id))
    );

    let mut dropped = Terminal::new(4, 3, TerminalConfig::default());
    dropped.set_cell_pixel_size(1, 6);
    dropped.advance(b"\x1b[?1049h\x1b[2;4H\x1bP7;0;0q\"1;1;1;6#1;2;100;0;0#1~\x1b\\");
    dropped.resize(2, 3);
    assert_eq!(dropped.image_metrics().placement_count, 0);
    assert_eq!(dropped.image_metrics().content_count, 0);
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
    assert_eq!(resized.image_placements().count(), 1);
    assert_eq!(resized.image_contents().count(), 1);

    terminal.advance(b"\x1bc");
    let reset = terminal.snapshot(SnapshotRequest::default());
    assert_eq!(reset.image_placements().count(), 0);
    assert_eq!(reset.image_contents().count(), 0);
}
