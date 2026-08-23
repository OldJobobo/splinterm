use base64::{Engine as _, engine::general_purpose::STANDARD};
use sha2::{Digest, Sha256};
use splinterm_terminal::{
    ActiveScreen, CellSnapshotContent, ImageSourceFormat, SharedKittyUploadBudget, SnapshotRequest,
    Terminal, TerminalConfig, TerminalDamage, TerminalEvent,
};

fn terminal() -> Terminal {
    let mut terminal = Terminal::new(8, 4, TerminalConfig::default());
    terminal.set_cell_pixel_size(2, 2);
    terminal
}

fn png(width: u32, height: u32, rgba: &[u8]) -> Vec<u8> {
    let mut bytes = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut bytes, width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder
            .write_header()
            .unwrap()
            .write_image_data(rgba)
            .unwrap();
    }
    bytes
}

fn command(metadata: &str, payload: &[u8], terminator: &[u8]) -> Vec<u8> {
    let mut sequence = format!("\x1b]1337;File={metadata}:").into_bytes();
    sequence.extend_from_slice(STANDARD.encode(payload).as_bytes());
    sequence.extend_from_slice(terminator);
    sequence
}

fn first_row_text(terminal: &Terminal) -> String {
    terminal
        .snapshot(SnapshotRequest::default())
        .visible_rows()
        .next()
        .unwrap()
        .cells()
        .map(|cell| match cell.content() {
            CellSnapshotContent::Empty | CellSnapshotContent::Spacer { .. } => ' ',
            CellSnapshotContent::Scalar(character) => character,
            CellSnapshotContent::Composed(characters) => characters[0],
        })
        .collect()
}

fn rejected(terminal: &mut Terminal) -> bool {
    terminal
        .drain_events()
        .any(|event| event == TerminalEvent::ImageRejected("iTerm2 inline image"))
}

#[test]
fn recorded_spec_fixture_executes_every_advertised_case() {
    let fixture: serde_json::Value = serde_json::from_str(include_str!(
        "../../../fixtures/terminal-images/v1/protocol-fixtures/iterm2-inline-v1.json"
    ))
    .unwrap();
    assert_eq!(
        fixture["schema"],
        "splinterm.phase5.iterm2-inline-fixtures.v1"
    );
    assert_eq!(
        fixture["source"]["retrieved_sha256"],
        "d339ce31f07475130bf0b73178d71291b5606ea1102ab8fd217b7f5ce6599961"
    );
    let context = &fixture["wire_context"];
    assert_eq!(context["screen_columns"], 8);
    assert_eq!(context["screen_rows"], 4);
    assert_eq!(context["cell_width_pixels"], 2);
    assert_eq!(context["cell_height_pixels"], 2);
    let png = STANDARD
        .decode(context["png_base64"].as_str().unwrap())
        .unwrap();
    assert_eq!(
        format!("{:x}", Sha256::digest(&png)),
        context["png_sha256"].as_str().unwrap()
    );
    let cases = fixture["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 12);
    for case in cases {
        let id = case["id"].as_str().unwrap();
        let metadata = case["metadata"]
            .as_str()
            .unwrap()
            .replace("{size}", &png.len().to_string());
        let mut terminal = terminal();
        if let Some(prelude) = case.get("prelude").and_then(|value| value.as_str()) {
            terminal.advance(prelude.as_bytes());
        }
        let terminator = match case["terminator_hex"].as_str().unwrap() {
            "07" => b"\x07".as_slice(),
            "1b5c" => b"\x1b\\".as_slice(),
            "9c" => b"\x9c".as_slice(),
            _ => panic!("unknown fixture terminator"),
        };
        terminal.advance(&command(&metadata, &png, terminator));
        let snapshot = terminal.snapshot(SnapshotRequest::default());
        if case["accepted"].as_bool().unwrap() {
            let placement = snapshot.image_placements().next().unwrap();
            assert_eq!(
                placement.destination.columns,
                usize::try_from(case["columns"].as_u64().unwrap()).unwrap(),
                "{id}"
            );
            assert_eq!(
                placement.destination.rows,
                usize::try_from(case["rows"].as_u64().unwrap()).unwrap(),
                "{id}"
            );
            assert_eq!(
                snapshot.cursor().cursor.position().row,
                i32::try_from(case["cursor_row"].as_i64().unwrap()).unwrap(),
                "{id}"
            );
            assert_eq!(
                snapshot.cursor().cursor.position().column,
                i32::try_from(case["cursor_column"].as_i64().unwrap()).unwrap(),
                "{id}"
            );
            assert!(!rejected(&mut terminal), "{id}");
        } else {
            assert_eq!(snapshot.image_contents().count(), 0, "{id}");
            assert!(rejected(&mut terminal), "{id}");
        }
    }
}

#[test]
fn inline_png_streams_into_the_generic_image_plane() {
    let png = png(2, 1, &[255, 0, 0, 255, 0, 255, 0, 128]);
    let sequence = command(
        &format!("name=dGVzdC5wbmc=;size={};inline=1", png.len()),
        &png,
        b"\x07",
    );
    let mut terminal = terminal();
    terminal.advance(&sequence);

    let snapshot = terminal.snapshot(SnapshotRequest::default());
    let content = snapshot.image_contents().next().unwrap();
    assert_eq!(content.source_format, ImageSourceFormat::Iterm2);
    assert_eq!((content.width, content.height), (2, 1));
    assert_eq!(
        terminal.image_content(content.id).unwrap().pixels(),
        &[0, 0, 255, 255, 0, 128, 0, 128]
    );
    let placement = snapshot.image_placements().next().unwrap();
    assert_eq!(
        (placement.destination.columns, placement.destination.rows),
        (1, 1)
    );
    assert_eq!(snapshot.cursor().cursor.position().row, 1);
    assert!(!rejected(&mut terminal));
}

#[test]
fn osc_1337_is_chunk_independent_and_accepts_st_and_c1_st() {
    let png = png(1, 1, &[1, 2, 3, 255]);
    for terminator in [b"\x1b\\".as_slice(), b"\x9c".as_slice()] {
        let sequence = command("inline=1;doNotMoveCursor=1", &png, terminator);
        for split in 0..=sequence.len() {
            let mut terminal = terminal();
            terminal.advance(&sequence[..split]);
            terminal.advance(&sequence[split..]);
            let snapshot = terminal.snapshot(SnapshotRequest::default());
            assert_eq!(snapshot.image_contents().count(), 1, "split {split}");
            assert_eq!(snapshot.image_placements().count(), 1, "split {split}");
            assert_eq!(snapshot.cursor().cursor.position().row, 0, "split {split}");
        }
    }

    let mut terminal = terminal();
    let mut c1 = b"\x9d1337;File=inline=1:".to_vec();
    c1.extend_from_slice(STANDARD.encode(&png).as_bytes());
    c1.push(0x9c);
    terminal.advance(&c1);
    assert_eq!(
        terminal
            .snapshot(SnapshotRequest::default())
            .image_contents()
            .count(),
        1
    );
}

#[test]
fn aspect_extents_and_cursor_policy_are_deterministic() {
    let png = png(2, 1, &[255; 8]);
    let mut preserved = terminal();
    preserved.advance(&command(
        "inline=1;width=2;height=2;preserveAspectRatio=1;doNotMoveCursor=1",
        &png,
        b"\x07",
    ));
    let snapshot = preserved.snapshot(SnapshotRequest::default());
    let placement = snapshot.image_placements().next().unwrap();
    assert_eq!(
        (placement.destination.columns, placement.destination.rows),
        (2, 1)
    );
    assert_eq!(snapshot.cursor().cursor.position().row, 0);

    let mut stretched = terminal();
    stretched.advance(&command(
        "inline=1;width=4px;height=4px;preserveAspectRatio=0",
        &png,
        b"\x07",
    ));
    let snapshot = stretched.snapshot(SnapshotRequest::default());
    let placement = snapshot.image_placements().next().unwrap();
    assert_eq!(
        (placement.destination.columns, placement.destination.rows),
        (2, 2)
    );
    assert_eq!(snapshot.cursor().cursor.position().row, 2);

    let mut percentage = terminal();
    percentage.advance(&command(
        "inline=1;width=50%;height=auto;doNotMoveCursor=1",
        &png,
        b"\x07",
    ));
    let snapshot = percentage.snapshot(SnapshotRequest::default());
    let placement = snapshot.image_placements().next().unwrap();
    assert_eq!(
        (placement.destination.columns, placement.destination.rows),
        (4, 2)
    );
}

#[test]
fn malformed_cancelled_and_budget_exhausted_transfers_fail_closed() {
    for metadata in [
        "inline=0",
        "inline=1;width=0%",
        "inline=1;unknown=1",
        "inline=1;inline=1",
    ] {
        let mut terminal = terminal();
        terminal.advance(&command(metadata, b"not a png", b"\x07"));
        assert!(rejected(&mut terminal), "{metadata}");
        assert_eq!(
            terminal
                .snapshot(SnapshotRequest::default())
                .image_contents()
                .count(),
            0
        );
    }

    let budget = SharedKittyUploadBudget::new(8 * 1024 * 1024);
    let mut terminal = Terminal::new(
        8,
        4,
        TerminalConfig {
            shared_kitty_upload_budget: Some(budget.clone()),
            ..TerminalConfig::default()
        },
    );
    terminal.set_cell_pixel_size(2, 2);
    terminal.advance(b"\x1b]1337;File=inline=1:AAAA");
    assert_eq!(budget.metrics().reserved_bytes, 8 * 1024 * 1024);
    terminal.advance(b"\x18after");
    assert_eq!(budget.metrics().reserved_bytes, 0);
    assert_eq!(
        terminal
            .snapshot(SnapshotRequest::default())
            .image_contents()
            .count(),
        0
    );

    let mut other = Terminal::new(
        8,
        4,
        TerminalConfig {
            shared_kitty_upload_budget: Some(SharedKittyUploadBudget::new(0)),
            ..TerminalConfig::default()
        },
    );
    other.set_cell_pixel_size(2, 2);
    other.advance(b"\x1b]1337;File=inline=1:AAAA\x07");
    assert!(rejected(&mut other));
}

#[test]
fn metadata_and_payload_exact_limits_preserve_accounting() {
    let png = png(1, 1, &[1, 2, 3, 255]);
    let encoded_name = STANDARD.encode(vec![b'A'; 756]);
    assert_eq!(encoded_name.len(), 1008);
    let mut exact_metadata = format!("inline=1;name={encoded_name};;");
    assert_eq!(exact_metadata.len(), 1024);
    let mut exact_terminal = terminal();
    exact_terminal.advance(&command(&exact_metadata, &png, b"\x07"));
    assert_eq!(
        exact_terminal
            .snapshot(SnapshotRequest::default())
            .image_contents()
            .count(),
        1
    );

    exact_metadata.push(';');
    let mut terminal = terminal();
    terminal.advance(&command(&exact_metadata, &png, b"\x07"));
    assert!(rejected(&mut terminal));
    assert_eq!(
        terminal
            .snapshot(SnapshotRequest::default())
            .image_contents()
            .count(),
        0
    );

    for extra in [0, 1] {
        let budget = SharedKittyUploadBudget::new(8 * 1024 * 1024);
        let mut terminal = Terminal::new(
            8,
            4,
            TerminalConfig {
                shared_kitty_upload_budget: Some(budget.clone()),
                ..TerminalConfig::default()
            },
        );
        terminal.set_cell_pixel_size(2, 2);
        terminal.advance(b"\x1b]1337;File=inline=1:");
        terminal.advance(&vec![b'A'; 8 * 1024 * 1024]);
        if extra == 1 {
            terminal.advance(b"A");
        }
        assert_eq!(budget.metrics().reserved_bytes, 8 * 1024 * 1024);
        terminal.advance(b"\x07");
        assert!(rejected(&mut terminal));
        assert_eq!(budget.metrics().reserved_bytes, 0);
        assert_eq!(budget.metrics().high_water_reserved_bytes, 8 * 1024 * 1024);
    }
}

#[test]
fn aggregate_admission_and_every_abort_path_release_and_resynchronize() {
    let budget = SharedKittyUploadBudget::new(16 * 1024 * 1024);
    let configured = || {
        let mut terminal = Terminal::new(
            8,
            4,
            TerminalConfig {
                shared_kitty_upload_budget: Some(budget.clone()),
                ..TerminalConfig::default()
            },
        );
        terminal.set_cell_pixel_size(2, 2);
        terminal
    };
    let mut first = configured();
    let mut second = configured();
    let mut rejected_terminal = configured();
    for terminal in [&mut first, &mut second, &mut rejected_terminal] {
        terminal.advance(b"\x1b]1337;File=inline=1:A");
    }
    assert_eq!(budget.metrics().reserved_bytes, 16 * 1024 * 1024);
    assert_eq!(budget.metrics().high_water_reserved_bytes, 16 * 1024 * 1024);
    rejected_terminal.advance(b"\x1aZ");
    assert!(first_row_text(&rejected_terminal).starts_with('Z'));
    first.advance(b"\x18Z");
    assert!(first_row_text(&first).starts_with('Z'));
    second.advance(b"\x1aZ");
    assert!(first_row_text(&second).starts_with('Z'));
    assert_eq!(budget.metrics().reserved_bytes, 0);

    for split in 0..=b"\x1b]1337;File=inline=1:AAAA\x1bxZ".len() {
        let mut terminal = configured();
        let sequence = b"\x1b]1337;File=inline=1:AAAA\x1bxZ";
        terminal.advance(&sequence[..split]);
        terminal.advance(&sequence[split..]);
        assert!(first_row_text(&terminal).starts_with('Z'), "split {split}");
        assert_eq!(budget.metrics().reserved_bytes, 0, "split {split}");
    }

    let mut dropped = configured();
    dropped.advance(b"\x1b]1337;File=inline=1:A");
    assert_eq!(budget.metrics().reserved_bytes, 8 * 1024 * 1024);
    drop(dropped);
    assert_eq!(budget.metrics().reserved_bytes, 0);
}

#[test]
fn all_terminators_release_admission_and_malformed_escape_can_start_kitty() {
    let png = png(1, 1, &[1, 2, 3, 255]);
    for terminator in [b"\x07".as_slice(), b"\x1b\\".as_slice(), b"\x9c".as_slice()] {
        let budget = SharedKittyUploadBudget::new(8 * 1024 * 1024);
        let mut terminal = Terminal::new(
            8,
            4,
            TerminalConfig {
                shared_kitty_upload_budget: Some(budget.clone()),
                ..TerminalConfig::default()
            },
        );
        terminal.set_cell_pixel_size(2, 2);
        terminal.advance(&command("inline=1", &png, terminator));
        assert_eq!(budget.metrics().reserved_bytes, 0);
        assert_eq!(budget.metrics().high_water_reserved_bytes, 8 * 1024 * 1024);
    }

    let budget = SharedKittyUploadBudget::new(8 * 1024 * 1024);
    let mut terminal = Terminal::new(
        8,
        4,
        TerminalConfig {
            shared_kitty_upload_budget: Some(budget.clone()),
            ..TerminalConfig::default()
        },
    );
    terminal.set_cell_pixel_size(2, 2);
    terminal.advance(b"\x1b]1337;File=inline=1:AAAA\x1b_Ga=T,f=32,s=1,v=1;/wAA/w==\x1b\\");
    let snapshot = terminal.snapshot(SnapshotRequest::default());
    assert_eq!(snapshot.image_contents().count(), 1);
    assert_eq!(
        snapshot.image_contents().next().unwrap().source_format,
        ImageSourceFormat::KittyRgba
    );
    assert_eq!(budget.metrics().reserved_bytes, 0);
}

#[test]
fn interleaving_aborts_the_prior_kitty_upload_and_keeps_one_pty_admission() {
    let budget = SharedKittyUploadBudget::new(16 * 1024 * 1024);
    let mut terminal = Terminal::new(
        8,
        4,
        TerminalConfig {
            shared_kitty_upload_budget: Some(budget.clone()),
            ..TerminalConfig::default()
        },
    );
    terminal.set_cell_pixel_size(2, 2);
    terminal.advance(b"\x1b_Ga=t,f=32,s=1,v=1,i=91,m=1;/wAA/w==\x1b\\");
    assert_eq!(budget.metrics().reserved_bytes, 4096);
    terminal.advance(b"\x1b]1337;File=inline=1:AAAA\x07");
    let events = terminal.drain_events().collect::<Vec<_>>();
    assert!(events.iter().any(|event| matches!(
        event,
        TerminalEvent::PtyWrite(reply)
            if reply.starts_with(b"\x1b_Gi=91;EINVAL:interleaved image upload")
    )));
    assert!(
        events
            .iter()
            .any(|event| event == &TerminalEvent::ImageRejected("iTerm2 inline image"))
    );
    assert_eq!(budget.metrics().reserved_bytes, 0);
}

#[test]
fn text_overwrite_reclaims_anonymous_inline_content() {
    let png = png(1, 1, &[1, 2, 3, 255]);
    let mut terminal = terminal();
    terminal.advance(&command("inline=1;doNotMoveCursor=1", &png, b"\x07"));
    terminal.advance(b"X");
    let snapshot = terminal.snapshot(SnapshotRequest::default());
    assert_eq!(snapshot.image_placements().count(), 0);
    assert_eq!(snapshot.image_contents().count(), 0);
}

#[test]
fn resize_scrollback_alternate_screen_replay_and_reset_preserve_lifecycle() {
    let png = png(1, 1, &[1, 2, 3, 255]);
    let mut terminal = terminal();
    let base = terminal.revision();
    terminal.advance(&command("inline=1;doNotMoveCursor=1", &png, b"\x07"));
    let committed = terminal.revision();
    assert!(committed > base);
    let updates = terminal.updates_since(base).unwrap();
    assert_eq!(updates.current(), committed);
    assert!(
        updates
            .updates()
            .any(|update| update.damage().any(|damage| damage
                == &TerminalDamage::Images {
                    screen: ActiveScreen::Normal,
                }))
    );

    terminal.resize(10, 5);
    assert_eq!(
        terminal
            .snapshot(SnapshotRequest::default())
            .image_placements()
            .count(),
        1
    );
    terminal.advance(b"\r\n1\r\n2\r\n3\r\n4\r\n5");
    assert_eq!(
        terminal
            .snapshot(SnapshotRequest {
                max_scrollback_rows: 8,
            })
            .image_placements()
            .count(),
        1
    );
    terminal.advance(b"\x1b[?1049h");
    assert_eq!(terminal.active_screen(), ActiveScreen::Alternate);
    assert_eq!(
        terminal
            .snapshot(SnapshotRequest::default())
            .image_placements()
            .count(),
        0
    );
    terminal.advance(b"\x1b[?1049l");
    assert_eq!(
        terminal
            .snapshot(SnapshotRequest {
                max_scrollback_rows: 8,
            })
            .image_placements()
            .count(),
        1
    );
    terminal.advance(b"\x1bc");
    let snapshot = terminal.snapshot(SnapshotRequest::default());
    assert_eq!(snapshot.image_placements().count(), 0);
    assert_eq!(snapshot.image_contents().count(), 0);

    let mut bounded = Terminal::new(
        8,
        4,
        TerminalConfig {
            update_history_limit: 1,
            ..TerminalConfig::default()
        },
    );
    bounded.set_cell_pixel_size(2, 2);
    let stale = bounded.revision();
    bounded.advance(&command("inline=1", &png, b"\x07"));
    bounded.advance(b"A");
    bounded.advance(b"B");
    assert!(bounded.updates_since(stale).is_err());
    let resnapshot = bounded.snapshot(SnapshotRequest::default());
    assert_eq!(resnapshot.image_contents().count(), 1);
    assert_eq!(resnapshot.image_placements().count(), 1);
}
