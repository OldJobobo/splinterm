use std::{fs, io::Write as _};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use flate2::{Compression, write::ZlibEncoder};
use sha2::{Digest, Sha256};
use splinterm_terminal::{
    ActiveScreen, ImageAlphaMode, ImageLimits, ImageSourceFormat, SharedImageBudget,
    SnapshotRequest, Terminal, TerminalConfig, TerminalDamage, TerminalEvent,
};

fn terminal() -> Terminal {
    let mut terminal = Terminal::new(8, 4, TerminalConfig::default());
    terminal.set_cell_pixel_size(2, 2);
    terminal
}

fn writes(terminal: &mut Terminal) -> Vec<Vec<u8>> {
    terminal
        .drain_events()
        .filter_map(|event| match event {
            TerminalEvent::PtyWrite(bytes) => Some(bytes),
            _ => None,
        })
        .collect()
}

#[test]
#[allow(
    clippy::too_many_lines,
    reason = "the executable table mirrors all 15 recorded contract cases and their distinct assertions"
)]
fn recorded_spec_fixture_manifest_executes_every_case() {
    let fixture: serde_json::Value = serde_json::from_str(include_str!(
        "../../../fixtures/terminal-images/v1/protocol-fixtures/kitty-static-v1.json"
    ))
    .unwrap();
    assert_eq!(
        fixture["schema"],
        "splinterm.phase5.kitty-static-fixtures.v1"
    );
    let cases = fixture["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 15);
    for case in cases {
        let id = case["id"].as_str().unwrap();
        let mut terminal = terminal();
        terminal.set_cell_pixel_size(4, 4);
        let provision = |terminal: &mut Terminal, image_id: u32| {
            terminal
                .advance(format!("\x1b_Ga=t,f=32,s=1,v=1,i={image_id};/wAA/w==\x1b\\").as_bytes());
            let _ = terminal.drain_events().count();
        };
        match id {
            "put-existing-with-placement"
            | "z-below-nondefault-background"
            | "z-above-text"
            | "delete-exact-placement"
            | "delete-all-visible-and-free-unreferenced"
            | "clear-reset-and-alternate-screen-lifecycle" => provision(&mut terminal, 7),
            "equal-z-orders-by-image-id" => {
                provision(&mut terminal, 7);
                provision(&mut terminal, 8);
            }
            _ => {}
        }
        if id == "delete-exact-placement" {
            terminal.advance(b"\x1b_Ga=p,i=7,p=9,C=1\x1b\\");
            let _ = terminal.drain_events().count();
        } else if id == "delete-all-visible-and-free-unreferenced" {
            terminal.advance(b"\x1b_Ga=p,i=7,p=1,C=1\x1b\\");
            let _ = terminal.drain_events().count();
        }
        if let Some(setup) = case.get("setup_inputs").and_then(|value| value.as_array()) {
            for input in setup {
                terminal.advance(input.as_str().unwrap().as_bytes());
                let _ = terminal.drain_events().count();
            }
        }
        if let Some(inputs) = case.get("inputs").and_then(|value| value.as_array()) {
            for input in inputs {
                terminal.advance(input.as_str().unwrap().as_bytes());
            }
        } else {
            terminal.advance(case["input"].as_str().unwrap().as_bytes());
        }
        if let Some(following) = case.get("following_input").and_then(|value| value.as_str()) {
            terminal.advance(following.as_bytes());
        }
        let replies = writes(&mut terminal);
        if let Some(expected) = case.get("expected_reply") {
            if let Some(expected) = expected.as_str() {
                assert_eq!(
                    replies.first().map(Vec::as_slice),
                    Some(expected.as_bytes()),
                    "{id}"
                );
            } else {
                assert!(replies.is_empty(), "{id}");
            }
        }
        if let Some(prefix) = case
            .get("expected_reply_prefix")
            .and_then(|value| value.as_str())
        {
            assert!(
                replies
                    .first()
                    .is_some_and(|reply| reply.starts_with(prefix.as_bytes())),
                "{id}"
            );
        }
        if let Some(sequences) = case
            .get("terminal_sequences")
            .and_then(|value| value.as_array())
        {
            for sequence in sequences {
                terminal.advance(sequence.as_str().unwrap().as_bytes());
            }
        }
        let snapshot = terminal.snapshot(SnapshotRequest::default());
        match id {
            "query-direct-rgb-supported" | "query-reply-precedes-da1" => {
                assert_eq!(snapshot.image_contents().count(), 0);
            }
            "transmit-rgba-one-pixel" => {
                let metadata = snapshot.image_contents().next().unwrap();
                assert_eq!((metadata.width, metadata.height), (1, 1));
                assert_eq!(
                    terminal.image_content(metadata.id).unwrap().pixels(),
                    &[0, 0, 255, 255]
                );
            }
            "put-existing-with-placement" => {
                let placement = snapshot.image_placements().next().unwrap();
                assert_eq!(
                    (
                        placement.application_image_id,
                        placement.application_placement_id
                    ),
                    (Some(7), Some(9))
                );
                assert_eq!(
                    (placement.destination.columns, placement.destination.rows),
                    (2, 1)
                );
                assert_eq!(
                    (placement.x_offset, placement.y_offset, placement.z_index),
                    (1, 2, -1)
                );
            }
            "z-below-nondefault-background" => assert_eq!(
                snapshot.image_placements().next().unwrap().z_index,
                -1_073_741_825
            ),
            "z-above-text" => assert_eq!(snapshot.image_placements().next().unwrap().z_index, 0),
            "equal-z-orders-by-image-id" => assert_eq!(
                snapshot
                    .image_placements()
                    .map(|placement| placement.application_image_id.unwrap())
                    .collect::<Vec<_>>(),
                vec![7, 8]
            ),
            "chunked-rgb-two-parts" => {
                let metadata = snapshot.image_contents().next().unwrap();
                assert_eq!((metadata.width, metadata.height), (2, 1));
                assert_eq!(
                    terminal.image_content(metadata.id).unwrap().pixels(),
                    &[0, 0, 255, 255, 0, 0, 255, 255]
                );
            }
            "delete-exact-placement" => {
                assert_eq!(snapshot.image_placements().count(), 0);
                assert_eq!(snapshot.image_contents().count(), 1);
            }
            "delete-all-visible-and-free-unreferenced"
            | "clear-reset-and-alternate-screen-lifecycle" => {
                assert_eq!(snapshot.image_placements().count(), 0);
                assert_eq!(snapshot.image_contents().count(), 0);
            }
            _ => {}
        }
    }
}

#[test]
fn spec_query_reply_precedes_following_da1_and_never_commits_content() {
    let mut terminal = terminal();
    terminal.advance(b"\x1b_Gi=31,s=1,v=1,a=q,t=d,f=24;/wAA\x1b\\\x1b[c");
    assert_eq!(
        writes(&mut terminal),
        vec![b"\x1b_Gi=31;OK\x1b\\".to_vec(), b"\x1b[?62;22c".to_vec()]
    );
    assert_eq!(terminal.image_metrics().content_count, 0);
}

#[test]
fn direct_rgba_transmit_premultiplies_to_canonical_bgra() {
    let mut terminal = terminal();
    terminal.advance(b"\x1b_Ga=t,f=32,s=1,v=1,i=7;/wAAfw==\x1b\\");
    assert_eq!(writes(&mut terminal), vec![b"\x1b_Gi=7;OK\x1b\\".to_vec()]);
    let snapshot = terminal.snapshot(SnapshotRequest::default());
    let metadata = snapshot.image_contents().next().unwrap();
    assert_eq!(metadata.source_format, ImageSourceFormat::KittyRgba);
    assert_eq!(metadata.alpha_mode, ImageAlphaMode::Premultiplied);
    assert_eq!(
        terminal.image_content(metadata.id).unwrap().pixels(),
        &[0, 0, 127, 127]
    );
}

#[test]
fn png_and_zlib_rgb_decode_through_the_narrow_supported_codecs() {
    let mut png_bytes = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut png_bytes, 1, 1);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().unwrap();
        writer.write_image_data(&[20, 40, 60, 128]).unwrap();
    }
    let mut terminal = terminal();
    terminal
        .advance(format!("\x1b_Ga=t,f=100,i=4;{}\x1b\\", STANDARD.encode(png_bytes)).as_bytes());
    let png_metadata = terminal
        .snapshot(SnapshotRequest::default())
        .image_contents()
        .next()
        .unwrap();
    assert_eq!(png_metadata.source_format, ImageSourceFormat::KittyPng);
    assert_eq!(
        terminal.image_content(png_metadata.id).unwrap().pixels(),
        &[30, 20, 10, 128]
    );

    let mut compressor = ZlibEncoder::new(Vec::new(), Compression::default());
    compressor.write_all(&[255, 0, 0]).unwrap();
    let compressed = compressor.finish().unwrap();
    terminal.advance(
        format!(
            "\x1b_Ga=t,f=24,s=1,v=1,o=z,i=5;{}\x1b\\",
            STANDARD.encode(compressed)
        )
        .as_bytes(),
    );
    let red = terminal
        .snapshot(SnapshotRequest::default())
        .image_contents()
        .find(|content| content.source_format == ImageSourceFormat::KittyRgb)
        .unwrap();
    assert_eq!(
        terminal.image_content(red.id).unwrap().pixels(),
        &[0, 0, 255, 255]
    );
}

#[test]
fn png_and_raw_axis_limits_are_rejected_before_canonical_output_allocation() {
    let mut png_bytes = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut png_bytes, 4_097, 1);
        encoder.set_color(png::ColorType::Grayscale);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().unwrap();
        writer.write_image_data(&vec![0; 4_097]).unwrap();
    }
    let mut terminal = terminal();
    terminal
        .advance(format!("\x1b_Ga=t,f=100,i=44;{}\x1b\\", STANDARD.encode(png_bytes)).as_bytes());
    assert!(writes(&mut terminal)[0].starts_with(b"\x1b_Gi=44;E2BIG:"));
    assert_eq!(terminal.image_metrics().content_count, 0);

    let raw = vec![0_u8; 4_097 * 3];
    terminal.advance(
        format!(
            "\x1b_Ga=t,f=24,s=4097,v=1,i=45;{}\x1b\\",
            STANDARD.encode(raw)
        )
        .as_bytes(),
    );
    assert!(writes(&mut terminal)[0].starts_with(b"\x1b_Gi=45;E2BIG:"));
    assert_eq!(terminal.image_metrics().content_count, 0);
}

#[test]
fn representative_unwrapped_stream_payload_can_exceed_chunk_limit() {
    let pixels = vec![0x7f_u8; 2_000 * 3];
    let mut terminal = terminal();
    terminal.advance(
        format!(
            "\x1b_Ga=t,f=24,s=2000,v=1,i=22,q=2;{}\x1b\\",
            STANDARD.encode(pixels)
        )
        .as_bytes(),
    );
    assert_eq!(terminal.image_metrics().content_count, 1);
    assert!(writes(&mut terminal).is_empty());
}

#[test]
fn recorded_kitten_and_chafa_static_streams_replay_without_protocol_errors() {
    for (trace, expected_digest) in [
        (
            include_bytes!("kitty-data/kitten-icat-0.48.0.bin").as_slice(),
            "5bd32c8a0182a44f06e344dd27ed186f173eb85be272b83586ff2137be684d19",
        ),
        (
            include_bytes!("kitty-data/chafa-1.18.2.bin").as_slice(),
            "9c6208eb51deba6a23d3b5a1f7fcfbc0dcb6b91280c437749d660aaed6d7282d",
        ),
    ] {
        assert_eq!(format!("{:x}", Sha256::digest(trace)), expected_digest);
        let mut terminal = Terminal::new(80, 24, TerminalConfig::default());
        terminal.set_cell_pixel_size(10, 20);
        terminal.advance(trace);
        let snapshot = terminal.snapshot(SnapshotRequest::default());
        assert_eq!(snapshot.image_contents().count(), 1);
        assert_eq!(snapshot.image_placements().count(), 1);
        assert!(writes(&mut terminal).is_empty());
    }
}

#[test]
fn representative_anonymous_transmit_and_display_streams_are_supported() {
    let mut terminal = terminal();
    terminal.advance(b"\x1b_Ga=T,f=32,s=2,v=1,c=2,r=1,m=1,q=2;/wAA/w==\x1b\\");
    terminal.advance(b"\x1b_Gm=0,q=2;/wAA/w==\x1b\\");
    let snapshot = terminal.snapshot(SnapshotRequest::default());
    assert_eq!(snapshot.image_contents().count(), 1);
    let placement = snapshot.image_placements().next().unwrap();
    assert_eq!(placement.application_image_id, None);
    assert_eq!(placement.application_placement_id, None);
    assert_eq!(placement.destination.columns, 2);
    assert!(writes(&mut terminal).is_empty());
}

#[test]
fn chunked_rgb_upload_places_with_exact_geometry_ids_and_cursor_policy() {
    let mut terminal = terminal();
    terminal.advance(b"\x1b_Ga=t,f=24,s=2,v=1,i=8,m=1;/wAA\x1b\\");
    assert!(writes(&mut terminal).is_empty());
    terminal.advance(b"\x1b_Gm=0;/wAA\x1b\\");
    assert_eq!(writes(&mut terminal), vec![b"\x1b_Gi=8;OK\x1b\\".to_vec()]);
    terminal.advance(b"\x1b_Ga=p,i=8,p=9,x=1,y=0,w=1,h=1,c=2,r=1,X=1,Y=1,C=1,z=-1\x1b\\");
    assert_eq!(
        writes(&mut terminal),
        vec![b"\x1b_Gi=8,p=9;OK\x1b\\".to_vec()]
    );
    let snapshot = terminal.snapshot(SnapshotRequest::default());
    let placement = snapshot.image_placements().next().unwrap();
    assert_eq!(placement.application_image_id, Some(8));
    assert_eq!(placement.application_placement_id, Some(9));
    assert_eq!((placement.source.x, placement.source.width), (1, 1));
    assert_eq!(
        (placement.destination.columns, placement.destination.rows),
        (2, 1)
    );
    assert_eq!(
        (placement.x_offset, placement.y_offset, placement.z_index),
        (1, 1, -1)
    );
    assert_eq!(snapshot.cursor().cursor.position().column, 0);
}

#[test]
fn shared_inbound_upload_budget_exhausts_across_terminals_and_releases_on_abort() {
    let budget = splinterm_terminal::SharedKittyUploadBudget::new(8_192);
    let make_terminal = || {
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
    let mut first = make_terminal();
    let mut second = make_terminal();
    let mut rejected = make_terminal();
    first.advance(b"\x1b_Ga=t,f=24,s=2,v=1,i=1,m=1;A");
    second.advance(b"\x1b_Ga=t,f=24,s=2,v=1,i=2,m=1;A");
    assert_eq!(budget.metrics().reserved_bytes, 8_192);
    rejected.advance(b"\x1b_Ga=t,f=24,s=1,v=1,i=3,m=1;AAAA\x1b\\");
    assert!(writes(&mut rejected)[0].starts_with(b"\x1b_Gi=3;ENOSPC:"));
    first.advance(b"\x18");
    assert_eq!(budget.metrics().reserved_bytes, 4_096);
    rejected.advance(b"\x1b_Ga=t,f=24,s=1,v=1,i=3,m=1;AAAA\x1b\\");
    assert!(writes(&mut rejected).is_empty());
    rejected.advance(b"\x18");
    drop(second);
    assert_eq!(budget.metrics().reserved_bytes, 0);
    assert_eq!(budget.metrics().high_water_reserved_bytes, 8_192);
}

#[test]
fn unequal_charge_retransmit_adjusts_shared_budget_atomically() {
    let budget = SharedImageBudget::new(8);
    let mut terminal = Terminal::new(
        8,
        4,
        TerminalConfig {
            image_limits: ImageLimits {
                bytes_per_content: 12,
                bytes_per_terminal: 12,
                ..ImageLimits::default()
            },
            shared_image_budget: Some(budget.clone()),
            ..TerminalConfig::default()
        },
    );
    terminal.set_cell_pixel_size(2, 2);
    terminal.advance(b"\x1b_Ga=t,f=32,s=2,v=1,i=7;/wAA//8AAP8=\x1b\\");
    assert_eq!(budget.metrics().content_bytes, 8);
    terminal.advance(b"\x1b_Ga=t,f=32,s=1,v=1,i=7;AP8A/w==\x1b\\");
    assert_eq!(budget.metrics().content_bytes, 4);
    terminal.advance(b"\x1b_Ga=t,f=32,s=2,v=1,i=7;/wAA//8AAP8=\x1b\\");
    assert_eq!(budget.metrics().content_bytes, 8);
    let before = terminal
        .snapshot(SnapshotRequest::default())
        .image_contents()
        .next()
        .unwrap();
    terminal.advance(b"\x1b_Ga=T,f=32,s=1,v=1,i=7,X=2;AP8A/w==\x1b\\");
    assert!(
        writes(&mut terminal)
            .iter()
            .any(|reply| reply.starts_with(b"\x1b_Gi=7;EINVAL:"))
    );
    assert_eq!(budget.metrics().content_bytes, 8);
    assert_eq!(
        terminal
            .snapshot(SnapshotRequest::default())
            .image_contents()
            .next()
            .unwrap(),
        before
    );
    terminal.advance(b"\x1b_Ga=t,f=32,s=3,v=1,i=7;/wAA//8AAP//AAD/\x1b\\");
    assert!(
        writes(&mut terminal)
            .iter()
            .any(|reply| reply.starts_with(b"\x1b_Gi=7;ENOSPC:"))
    );
    let after = terminal
        .snapshot(SnapshotRequest::default())
        .image_contents()
        .next()
        .unwrap();
    assert_eq!(after, before);
    assert_eq!(budget.metrics().content_bytes, 8);
}

#[test]
fn same_charge_retransmit_reuses_full_shared_budget_and_removes_old_placements() {
    let budget = SharedImageBudget::new(4);
    let mut terminal = Terminal::new(
        8,
        4,
        TerminalConfig {
            image_limits: ImageLimits {
                bytes_per_content: 4,
                bytes_per_terminal: 4,
                ..ImageLimits::default()
            },
            shared_image_budget: Some(budget.clone()),
            ..TerminalConfig::default()
        },
    );
    terminal.set_cell_pixel_size(2, 2);
    terminal.advance(b"\x1b_Ga=T,f=32,s=1,v=1,i=7,p=9,C=1;/wAA/w==\x1b\\");
    terminal.advance(b"\x1b_Ga=t,f=32,s=1,v=1,i=7;AP8A/w==\x1b\\");
    let snapshot = terminal.snapshot(SnapshotRequest::default());
    assert_eq!(snapshot.image_contents().count(), 1);
    assert_eq!(snapshot.image_placements().count(), 0);
    let metadata = snapshot.image_contents().next().unwrap();
    assert_eq!(
        terminal.image_content(metadata.id).unwrap().pixels(),
        &[0, 255, 0, 255]
    );
    assert_eq!(budget.metrics().content_bytes, 4);
}

#[test]
fn transmit_and_display_moves_cursor_and_same_pair_replaces_placement() {
    let mut terminal = terminal();
    terminal.advance(b"\x1b_Ga=T,f=32,s=1,v=1,i=7,p=9,c=2,r=1;/wAA/w==\x1b\\");
    let first = terminal.snapshot(SnapshotRequest::default());
    assert_eq!(first.image_placements().count(), 1);
    assert_eq!(
        (
            first.cursor().cursor.position().column,
            first.cursor().cursor.position().row,
        ),
        (2, 1)
    );
    terminal.advance(b"\x1b_Ga=p,i=7,p=9,c=1,r=1,C=1,z=4\x1b\\");
    let replaced = terminal.snapshot(SnapshotRequest::default());
    assert_eq!(replaced.image_placements().count(), 1);
    assert_eq!(replaced.image_placements().next().unwrap().z_index, 4);
}

#[test]
fn z_tiers_and_equal_z_application_ids_have_deterministic_order() {
    let mut terminal = terminal();
    for command in [
        b"\x1b_Ga=t,f=32,s=1,v=1,i=8;/wAA/w==\x1b\\".as_slice(),
        b"\x1b_Ga=t,f=32,s=1,v=1,i=7;/wAA/w==\x1b\\".as_slice(),
        b"\x1b_Ga=p,i=8,p=1,C=1,z=-1\x1b\\".as_slice(),
        b"\x1b_Ga=p,i=7,p=1,C=1,z=-1\x1b\\".as_slice(),
        b"\x1b_Ga=p,i=7,p=2,C=1,z=-1073741825\x1b\\".as_slice(),
        b"\x1b_Ga=p,i=8,p=2,C=1,z=0\x1b\\".as_slice(),
    ] {
        terminal.advance(command);
    }
    let ids = terminal
        .snapshot(SnapshotRequest::default())
        .image_placements()
        .map(|placement| placement.application_image_id.unwrap())
        .collect::<Vec<_>>();
    assert_eq!(ids, vec![7, 7, 8, 8]);
    let z_values = terminal
        .snapshot(SnapshotRequest::default())
        .image_placements()
        .map(|placement| placement.z_index)
        .collect::<Vec<_>>();
    assert_eq!(z_values, vec![-1_073_741_825, -1, -1, 0]);
}

#[test]
fn missing_delete_unsupported_and_quiet_errors_are_bounded_and_compatible() {
    let mut terminal = terminal();
    terminal.advance(b"\x1b_Ga=p,i=4294967295\x1b\\");
    assert!(writes(&mut terminal)[0].starts_with(b"\x1b_Gi=4294967295;ENOENT:"));
    terminal.advance(b"\x1b_Ga=q,t=f,f=100,i=12;L3RtcA==\x1b\\");
    assert!(writes(&mut terminal)[0].starts_with(b"\x1b_Gi=12;ENOTSUP:"));
    terminal.advance(b"\x1b_Ga=f,f=32,s=1,v=1,i=7;/wAA/w==\x1b\\");
    assert!(writes(&mut terminal)[0].starts_with(b"\x1b_Gi=7;ENOTSUP:"));
    terminal.advance(b"\x1b_Ga=p,i=4294967295,q=2\x1b\\");
    assert!(writes(&mut terminal).is_empty());
}

#[test]
fn exact_and_visible_delete_follow_lowercase_uppercase_retention() {
    let mut terminal = terminal();
    terminal.advance(b"\x1b_Ga=T,f=32,s=1,v=1,i=7,p=9,C=1;/wAA/w==\x1b\\");
    terminal.advance(b"\x1b_Ga=d,d=i,i=7,p=9\x1b\\");
    let placement_only = terminal.snapshot(SnapshotRequest::default());
    assert_eq!(placement_only.image_placements().count(), 0);
    assert_eq!(placement_only.image_contents().count(), 1);
    terminal.advance(b"\x1b_Ga=p,i=7,p=10,C=1\x1b\\");
    terminal.advance(b"\x1b_Ga=d,d=A\x1b\\");
    let deleted = terminal.snapshot(SnapshotRequest::default());
    assert_eq!(deleted.image_placements().count(), 0);
    assert_eq!(deleted.image_contents().count(), 0);
}

#[test]
fn visible_delete_and_ed2_preserve_scrollback_placements_and_content() {
    let mut terminal = terminal();
    terminal.advance(b"\x1b_Ga=T,f=32,s=1,v=1,i=1,p=1,C=1;/wAA/w==\x1b\\");
    terminal.advance(b"\r\n1\r\n2\r\n3\r\n4");
    terminal.advance(b"\x1b_Ga=T,f=32,s=1,v=1,i=2,p=1,C=1;AP8A/w==\x1b\\");
    terminal.advance(b"\x1b_Ga=d,d=A\x1b\\");
    let deleted = terminal.snapshot(SnapshotRequest::default());
    assert!(
        deleted
            .image_placements()
            .any(|placement| placement.application_image_id == Some(1))
    );
    assert!(
        !deleted
            .image_placements()
            .any(|placement| placement.application_image_id == Some(2))
    );
    assert_eq!(deleted.image_contents().count(), 1);

    terminal.advance(b"\x1b[2J");
    let cleared = terminal.snapshot(SnapshotRequest::default());
    assert!(
        cleared
            .image_placements()
            .any(|placement| placement.application_image_id == Some(1))
    );
    assert_eq!(cleared.image_contents().count(), 1);
}

#[test]
fn crop_intersection_aspect_extent_and_offset_bounds_match_kitty() {
    let mut terminal = terminal();
    let pixels = vec![255_u8; 4 * 2 * 4];
    terminal.advance(
        format!(
            "\x1b_Ga=t,f=32,s=4,v=2,i=7;{}\x1b\\",
            STANDARD.encode(pixels)
        )
        .as_bytes(),
    );
    terminal.advance(b"\x1b_Ga=p,i=7,p=1,x=3,y=0,w=9,h=2,c=2,C=1,X=1,Y=1\x1b\\");
    let clipped = terminal.snapshot(SnapshotRequest::default());
    let placement = clipped.image_placements().next().unwrap();
    assert_eq!((placement.source.x, placement.source.width), (3, 1));
    assert_eq!(
        (placement.destination.columns, placement.destination.rows),
        (2, 4)
    );
    terminal.advance(b"\x1b_Ga=p,i=7,p=2,x=3,w=1,h=2,r=2,C=1\x1b\\");
    let one_sided = terminal.snapshot(SnapshotRequest::default());
    assert_eq!(
        one_sided
            .image_placements()
            .find(|placement| placement.application_placement_id == Some(2))
            .unwrap()
            .destination
            .columns,
        1
    );
    terminal.advance(b"\x1b_Ga=p,i=7,p=3,C=1,X=2\x1b\\");
    assert!(
        writes(&mut terminal)
            .iter()
            .any(|reply| reply.starts_with(b"\x1b_Gi=7,p=3;EINVAL:"))
    );
    assert!(
        !terminal
            .snapshot(SnapshotRequest::default())
            .image_placements()
            .any(|placement| placement.application_placement_id == Some(3))
    );
}

#[test]
fn continuation_failures_keep_initial_correlation_and_quiet_policy() {
    let mut terminal = terminal();
    terminal.advance(b"\x1b_Ga=t,f=24,s=2,v=1,i=8,m=1;/wAA\x1b\\");
    terminal.advance(b"\x1b_Gm=0;%%%\x1b\\");
    assert!(writes(&mut terminal)[0].starts_with(b"\x1b_Gi=8;EBADMSG:"));
    terminal.advance(b"\x1b_Ga=t,f=24,s=2,v=1,i=9,m=1;/wAA\x1b\\");
    terminal.advance(b"\x1b_Gm=x;AAAA\x1b\\");
    assert!(writes(&mut terminal)[0].starts_with(b"\x1b_Gi=9;EINVAL:"));
    terminal.advance(b"\x1b_Ga=t,f=24,s=2,v=1,i=10,m=1;/wAA\x1b\\");
    terminal.advance(b"\x1b_Gm=0,q=2;%%%\x1b\\");
    assert!(writes(&mut terminal).is_empty());
}

#[test]
fn unrelated_apc_is_ignored_and_kitty_is_chunk_independent() {
    let input = b"A\x1b_not-kitty\x1b\\B\x1b_other\x9cD\x9fignored\x1b\\E\x1b_Gi=31,s=1,v=1,a=q,t=d,f=24;/wAA\x1b\\C";
    let mut expected = terminal();
    expected.advance(input);
    for split in 0..=input.len() {
        let mut actual = terminal();
        actual.advance(&input[..split]);
        actual.advance(&input[split..]);
        assert_eq!(actual, expected, "split {split}");
    }
}

#[test]
fn abort_and_invalid_continuation_discard_partial_upload_without_state() {
    let mut terminal = terminal();
    terminal.advance(b"\x1b_Ga=t,f=24,s=2,v=1,i=8,m=1;/wAA\x18Z");
    assert_eq!(terminal.image_metrics().content_count, 0);
    terminal.advance(b"\x1b_Ga=t,f=24,s=2,v=1,i=8,m=1;/wAA\x1b\\");
    terminal.advance(b"\x1b_Gm=0,i=9;/wAA\x1b\\");
    assert!(
        writes(&mut terminal)
            .iter()
            .any(|reply| reply.starts_with(b"\x1b_Gi=8;EINVAL:"))
    );
    assert_eq!(terminal.image_metrics().content_count, 0);

    terminal.advance(b"\x1b_Ga=T,f=32,s=1,v=1,i=7,p=9,C=1;/wAA/w==\x1b\\");
    terminal.advance(b"\x1b_Ga=t,f=24,s=2,v=1,i=8,m=1;/wAA\x1b\\");
    terminal.advance(b"\x1b_Ga=d,d=i,i=7,p=9\x1b\\");
    assert_eq!(
        terminal
            .snapshot(SnapshotRequest::default())
            .image_placements()
            .count(),
        0
    );
    terminal.advance(b"\x1b_Gm=0;/wAA\x1b\\");
    assert_eq!(terminal.image_metrics().content_count, 1);
}

#[test]
fn resize_scrollback_screen_switch_and_revision_replay_preserve_kitty_identity() {
    let mut terminal = terminal();
    terminal.advance(b"\x1b_Ga=T,f=32,s=1,v=1,i=7,p=9,C=1;/wAA/w==\x1b\\");
    let base = terminal.revision();
    let original = terminal
        .snapshot(SnapshotRequest::default())
        .image_contents()
        .next()
        .unwrap();
    terminal.resize(10, 5);
    terminal.advance(b"\r\n1\r\n2\r\n3\r\n4\r\n5\r\n6");
    let scrolled = terminal.snapshot(SnapshotRequest::default());
    assert!(
        scrolled
            .image_placements()
            .any(|placement| placement.application_image_id == Some(7))
    );
    assert_eq!(scrolled.image_contents().next().unwrap().id, original.id);
    assert!(
        terminal
            .updates_since(base)
            .unwrap()
            .updates()
            .any(|update| update
                .damage()
                .any(|damage| matches!(damage, TerminalDamage::Images { .. })))
    );

    terminal.advance(b"\x1b[?1049h\x1b[?1049l\x1b_Ga=p,i=7,p=10,C=1\x1b\\");
    let reattached = terminal.snapshot(SnapshotRequest::default());
    assert_eq!(reattached.image_contents().next().unwrap().id, original.id);
    assert!(
        reattached
            .image_placements()
            .any(|placement| placement.application_placement_id == Some(10))
    );
}

#[test]
fn external_media_never_open_unlink_or_consume_application_named_objects() {
    let root =
        std::env::temp_dir().join(format!("splinterm-kitty-external-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir(&root).unwrap();
    let target = root.join("sentinel.png");
    let link = root.join("replacement.png");
    fs::write(&target, b"must remain unread and unchanged").unwrap();
    std::os::unix::fs::symlink(&target, &link).unwrap();

    for (index, medium, named_object) in [
        (81, 'f', target.as_os_str().as_encoded_bytes()),
        (82, 't', link.as_os_str().as_encoded_bytes()),
        (83, 's', b"/splinterm-missing-shm".as_slice()),
    ] {
        let mut terminal = terminal();
        terminal.advance(
            format!(
                "\x1b_Ga=T,t={medium},f=100,i={index};{}\x1b\\",
                STANDARD.encode(named_object)
            )
            .as_bytes(),
        );
        let reply = writes(&mut terminal).remove(0);
        assert!(reply.starts_with(format!("\x1b_Gi={index};ENOTSUP:").as_bytes()));
        assert_eq!(
            terminal
                .snapshot(SnapshotRequest::default())
                .image_contents()
                .count(),
            0
        );
    }
    assert_eq!(
        fs::read(&target).unwrap(),
        b"must remain unread and unchanged"
    );
    assert_eq!(fs::read_link(&link).unwrap(), target);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn clear_reset_and_fresh_alternate_apply_kitty_lifecycle() {
    let mut terminal = terminal();
    terminal.advance(b"\x1b_Ga=T,f=32,s=1,v=1,i=7,p=9,C=1;/wAA/w==\x1b\\\x1b[2J");
    let cleared = terminal.snapshot(SnapshotRequest::default());
    assert_eq!(cleared.image_placements().count(), 0);
    assert_eq!(cleared.image_contents().count(), 1);
    terminal.advance(b"\x1b_Ga=p,i=7,p=10,C=1\x1b\\\x1b[?1049h");
    assert_eq!(terminal.active_screen(), ActiveScreen::Alternate);
    assert_eq!(
        terminal
            .snapshot(SnapshotRequest::default())
            .image_placements()
            .count(),
        0
    );
    terminal.advance(b"\x1b[?1049l\x1bc");
    assert_eq!(terminal.image_metrics().content_count, 0);
}
