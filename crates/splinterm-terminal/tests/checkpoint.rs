use splinterm_terminal::{
    CheckpointError, ImageAlphaMode, ImageRetention, ImageSourceFormat,
    MAX_TERMINAL_CHECKPOINT_BYTES, NewImageContent, SharedImageBudget, SharedKittyUploadBudget,
    SnapshotRequest, Terminal, TerminalConfig, TerminalEvent,
};

fn terminal() -> (Terminal, TerminalConfig) {
    let config = TerminalConfig::default();
    (Terminal::new(12, 4, config.clone()), config)
}

#[test]
fn canonical_round_trip_preserves_state_and_pending_replies() {
    let (mut original, config) = terminal();
    original.advance(
        b"one\r\ntwo\r\nthree\r\nfour\r\nfive\x1b[2;4r\x1b[3;1H\x1b[1;3;4:3m\x1b]2;checkpoint-title\x07\x1b[6n",
    );
    original.advance("e\u{301}".as_bytes());

    let bytes = original.encode_checkpoint().unwrap();
    assert!(bytes.len() < MAX_TERMINAL_CHECKPOINT_BYTES);
    let mut restored = Terminal::restore_checkpoint(&bytes, config).unwrap();
    assert_eq!(restored.encode_checkpoint().unwrap(), bytes);
    assert_eq!(restored.revision(), original.revision());
    assert_eq!(
        restored.drain_events().collect::<Vec<_>>(),
        original.drain_events().collect::<Vec<_>>()
    );
    assert_eq!(
        restored.encode_checkpoint().unwrap(),
        original.encode_checkpoint().unwrap()
    );

    let request = SnapshotRequest {
        max_scrollback_rows: 32,
    };
    assert_eq!(
        format!("{:?}", restored.snapshot(request)),
        format!("{:?}", original.snapshot(request))
    );
}

#[test]
fn every_supported_byte_split_continues_identically() {
    let fixtures: &[&[u8]] = &[
        "plain-\u{00e9}-e\u{301}-\u{754c}".as_bytes(),
        b"\x1b[1;3;4:3;38:2::12:34:56mstyled\x1b[0m",
        b"\x1b]2;split-title\x1b\\after",
        // C1 ST (0x9c) is also a UTF-8 continuation byte: U+4F5C is e4 bd 9c.
        b"\x1b]2;\xe4\xbd\x9c\xe6\xa5\xad\x9cafter",
        b"\x1b]4;7;rgb:12/34/56\x07palette",
        b"\x1bP1;2;3zignored-dcs\x1b\\done",
        b"\x1b_ignored-apc\x1b\\done",
        b"\x1b[?1049halt\x1b[?1049lback",
        b"\x1b[3;8H\x1b[6nquery",
        // More colon subparameters than the parser keeps.
        b"\x1b[4:1:1:1:1:1:1:1:1:1:1:1:1:1:1:1:1:1:1:1mover",
    ];

    for fixture in fixtures {
        for split in 0..=fixture.len() {
            let (mut uninterrupted, config) = terminal();
            uninterrupted.advance(&fixture[..split]);
            let prefix_events = uninterrupted.drain_events().collect::<Vec<_>>();
            let checkpoint = uninterrupted.encode_checkpoint().unwrap_or_else(|error| {
                panic!("fixture {fixture:?} split {split} failed capture: {error}")
            });
            let mut restored = Terminal::restore_checkpoint(&checkpoint, config).unwrap();
            assert_eq!(restored.drain_events().count(), 0);

            uninterrupted.advance(&fixture[split..]);
            restored.advance(&fixture[split..]);
            let uninterrupted_events = uninterrupted.drain_events().collect::<Vec<_>>();
            let restored_events = restored.drain_events().collect::<Vec<_>>();
            assert_eq!(restored_events, uninterrupted_events, "split {split}");
            assert_eq!(
                restored.encode_checkpoint().unwrap(),
                uninterrupted.encode_checkpoint().unwrap(),
                "split {split}, prefix events {prefix_events:?}"
            );
            assert_eq!(restored.revision(), uninterrupted.revision());
        }
    }
}

#[test]
fn malformed_noncanonical_corrupt_and_oversized_inputs_fail_closed() {
    let (terminal, config) = terminal();
    let bytes = terminal.encode_checkpoint().unwrap();

    for length in [0, 1, bytes.len() / 4, bytes.len() / 2, bytes.len() - 1] {
        assert!(Terminal::restore_checkpoint(&bytes[..length], config.clone()).is_err());
    }
    let mut whitespace = bytes.clone();
    whitespace.push(b'\n');
    assert!(matches!(
        Terminal::restore_checkpoint(&whitespace, config.clone()),
        Err(CheckpointError::NonCanonical)
    ));
    for index in (0..bytes.len()).step_by(64) {
        let mut corrupt = bytes.clone();
        corrupt[index] ^= 1;
        assert!(
            Terminal::restore_checkpoint(&corrupt, config.clone()).is_err(),
            "corrupt byte {index}"
        );
    }
    let marker = b"\"rows\":[";
    let start = bytes
        .windows(marker.len())
        .position(|window| window == marker)
        .unwrap()
        + marker.len();
    let mut excessive_rows = Vec::with_capacity(bytes.len() + 5_500_005);
    excessive_rows.extend_from_slice(&bytes[..start]);
    excessive_rows.extend_from_slice("null,".repeat(1_100_000).as_bytes());
    excessive_rows.extend_from_slice(&bytes[start..]);
    assert!(matches!(
        Terminal::restore_checkpoint(&excessive_rows, config.clone()),
        Err(CheckpointError::Encoding(_))
    ));
    assert!(matches!(
        Terminal::restore_checkpoint(&vec![b'x'; MAX_TERMINAL_CHECKPOINT_BYTES + 1], config),
        Err(CheckpointError::TooLarge)
    ));
}

#[test]
fn configuration_drift_and_unsupported_runtime_state_are_rejected() {
    let (baseline, mut changed_config) = terminal();
    let bytes = baseline.encode_checkpoint().unwrap();
    changed_config.osc_limit += 1;
    assert!(matches!(
        Terminal::restore_checkpoint(&bytes, changed_config),
        Err(CheckpointError::IncompatibleConfig)
    ));

    for prefix in [
        b"\x1b[?2026h".as_slice(),
        b"\x1bPq".as_slice(),
        b"\x1b_Ga=T,f=24,s=1,v=1;".as_slice(),
        b"\x1b]1337;File=name=x:".as_slice(),
    ] {
        let (mut active, _) = terminal();
        active.advance(prefix);
        assert!(matches!(
            active.encode_checkpoint(),
            Err(CheckpointError::UnsupportedState)
        ));
    }

    let (mut unsupported_event, _) = terminal();
    unsupported_event.advance(b"\x1bPzunsupported\x1b\\");
    assert!(matches!(
        unsupported_event.encode_checkpoint(),
        Err(CheckpointError::UnsupportedState)
    ));
}

#[test]
fn oversized_queued_title_event_fails_capture() {
    let oversized_title = "x".repeat(64 * 1024 + 1);
    let config = TerminalConfig {
        osc_limit: oversized_title.len() + 16,
        ..TerminalConfig::default()
    };
    let mut terminal = Terminal::new(4, 2, config);
    terminal.advance(format!("\x1b]2;{oversized_title}\x07\x1b]2;short\x07").as_bytes());

    assert!(matches!(
        terminal.encode_checkpoint(),
        Err(CheckpointError::InvalidState)
    ));
}

#[test]
fn shared_runtime_budgets_are_rebound_from_matching_restore_config() {
    let config = TerminalConfig {
        shared_image_budget: Some(SharedImageBudget::new(1024)),
        shared_kitty_upload_budget: Some(SharedKittyUploadBudget::new(1024)),
        ..TerminalConfig::default()
    };
    let mut original = Terminal::new(6, 2, config.clone());
    original.advance(b"budgeted-text");
    let bytes = original.encode_checkpoint().unwrap();
    let restored = Terminal::restore_checkpoint(&bytes, config.clone()).unwrap();
    assert_eq!(restored.encode_checkpoint().unwrap(), bytes);

    let changed_limits = TerminalConfig {
        shared_image_budget: Some(SharedImageBudget::new(2048)),
        shared_kitty_upload_budget: Some(SharedKittyUploadBudget::new(2048)),
        ..config
    };
    assert!(matches!(
        Terminal::restore_checkpoint(&bytes, changed_limits),
        Err(CheckpointError::IncompatibleConfig)
    ));
}

#[test]
fn image_history_and_event_overflow_remain_explicitly_unsupported() {
    let (mut image_terminal, _) = terminal();
    let image = image_terminal
        .insert_image_content(NewImageContent {
            width: 1,
            height: 1,
            source_format: ImageSourceFormat::KittyRgba,
            alpha_mode: ImageAlphaMode::Premultiplied,
            pixels: &[0, 0, 0, 0],
            retention: ImageRetention::ExplicitDelete,
        })
        .unwrap();
    image_terminal.remove_image_content(image).unwrap();
    assert!(matches!(
        image_terminal.encode_checkpoint(),
        Err(CheckpointError::UnsupportedState)
    ));

    let overflow_config = TerminalConfig {
        event_limit: 1,
        ..TerminalConfig::default()
    };
    let mut overflow = Terminal::new(4, 2, overflow_config);
    overflow.advance(b"\x07\x07");
    assert!(matches!(
        overflow.encode_checkpoint(),
        Err(CheckpointError::UnsupportedState)
    ));
}

#[test]
fn restored_pending_query_reply_remains_in_parser_order() {
    let (mut original, config) = terminal();
    original.advance(b"A\x1b[6nB\x1b[5nC");
    let bytes = original.encode_checkpoint().unwrap();
    let mut restored = Terminal::restore_checkpoint(&bytes, config).unwrap();
    let events = restored.drain_events().collect::<Vec<_>>();
    assert!(events.iter().any(
        |event| matches!(event, TerminalEvent::PtyWrite(bytes) if bytes.starts_with(b"\x1b["))
    ));
    assert_eq!(events, original.drain_events().collect::<Vec<_>>());
}

#[test]
fn checkpoints_fit_full_terminals_with_deep_scrollback() {
    // A full 200x50 screen with 10,000 lines of history must stay well inside
    // the checkpoint budget, or every handoff would be refused.
    let config = TerminalConfig {
        scrollback_lines: 10_000,
        ..TerminalConfig::default()
    };
    let mut terminal = Terminal::new(200, 50, config.clone());
    let line = format!("{}\r\n", "x".repeat(200));
    for _ in 0..10_050 {
        terminal.advance(line.as_bytes());
    }
    let bytes = terminal.encode_checkpoint().unwrap();
    assert!(
        bytes.len() < MAX_TERMINAL_CHECKPOINT_BYTES * 3 / 4,
        "{} bytes",
        bytes.len()
    );
    let restored = Terminal::restore_checkpoint(&bytes, config).unwrap();
    assert_eq!(restored.encode_checkpoint().unwrap(), bytes);

    // The protocol allows 1,000,000 scrollback lines; a mostly empty history
    // of that capacity must still checkpoint.
    let config = TerminalConfig {
        scrollback_lines: 1_000_000,
        ..TerminalConfig::default()
    };
    let mut terminal = Terminal::new(80, 24, config.clone());
    terminal.advance(b"short history\r\n");
    let bytes = terminal.encode_checkpoint().unwrap();
    Terminal::restore_checkpoint(&bytes, config).unwrap();
}
