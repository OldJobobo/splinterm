//! Deterministic JSON size evidence for the maximum proposed Beta1 terminal grid.

use serde_json::{Value, json};

const COLUMNS: usize = 480;
const ROWS: usize = 128;
const SNAPSHOT_SCROLLBACK_ROWS: usize = 16;
const MIB: usize = 1024 * 1024;

#[derive(Clone, Copy)]
enum CellProfile {
    Empty,
    Scalar,
    Composed,
    StyledScalar,
    StyledComposed,
}

impl CellProfile {
    const ALL: [Self; 5] = [
        Self::Empty,
        Self::Scalar,
        Self::Composed,
        Self::StyledScalar,
        Self::StyledComposed,
    ];

    const fn name(self) -> &'static str {
        match self {
            Self::Empty => "empty",
            Self::Scalar => "scalar",
            Self::Composed => "composed_64_chars",
            Self::StyledScalar => "styled_scalar",
            Self::StyledComposed => "styled_composed_64_chars",
        }
    }

    fn content(self) -> String {
        match self {
            Self::Empty => String::new(),
            Self::Scalar | Self::StyledScalar => "x".to_owned(),
            Self::Composed | Self::StyledComposed => {
                let mut content = String::from('x');
                content.extend(std::iter::repeat_n('\u{e0100}', 63));
                content
            }
        }
    }

    const fn styled(self) -> bool {
        matches!(self, Self::StyledScalar | Self::StyledComposed)
    }
}

fn cell(profile: CellProfile) -> Value {
    let content = profile.content();
    let mut cell = serde_json::Map::new();
    if !content.is_empty() {
        cell.insert("content".to_owned(), Value::String(content));
    }
    if profile.styled() {
        cell.insert(
            "attributes".to_owned(),
            json!([
                0x7f,
                3,
                3,
                0x00ff_ffff_u32,
                3,
                0x00ff_ffff_u32,
                3,
                0x00ff_ffff_u32,
            ]),
        );
    }
    Value::Object(cell)
}

fn rows(count: usize, profile: CellProfile, first_row_id: u64) -> Vec<Value> {
    let cells = vec![cell(profile); COLUMNS];
    (0..count)
        .map(|index| {
            json!({
                "row_id": first_row_id + u64::try_from(index).unwrap(),
                "cells": cells,
            })
        })
        .collect()
}

fn snapshot(profile: CellProfile) -> Value {
    json!({
        "splint_id": "00000000-0000-0000-0000-000000000001",
        "incarnation": 1,
        "revision": 1,
        "columns": COLUMNS,
        "rows": ROWS,
        "cursor_column": 479,
        "cursor_row": 127,
        "cursor_deferred_wrap": false,
        "active_screen": "normal",
        "input_modes": {
            "application_cursor": false,
            "application_keypad": false,
            "focus_reporting": false,
            "bracketed_paste": false,
            "cursor_visible": true,
            "cursor_blink": false,
            "mouse_tracking": "none",
            "sgr_mouse": false,
        },
        "palette": vec![0_u32; 256],
        "default_colors": [0, 0, 0],
        "title": "",
        "visible_rows": rows(ROWS, profile, 17),
        "history_generation": 1,
        "oldest_available_scrollback_row_id": 1,
        "newest_available_scrollback_row_id": 16,
        "scrollback_rows": rows(SNAPSHOT_SCROLLBACK_ROWS, profile, 1),
        "available_scrollback_rows": SNAPSHOT_SCROLLBACK_ROWS,
        "omitted_oldest_scrollback_rows": 0,
        "exited_code": null,
        "exited_signal": null,
    })
}

fn update(profile: CellProfile) -> Value {
    let rows = rows(ROWS, profile, 1)
        .into_iter()
        .enumerate()
        .map(|(index, row)| json!({ "index": index, "row": row }))
        .collect::<Vec<_>>();
    json!({
        "base_revision": 1,
        "revision": 2,
        "rows": rows,
        "scrolls": [],
        "cursor": null,
        "title": null,
        "input_modes": null,
        "active_screen": null,
        "palette": null,
        "default_colors": null,
        "columns": COLUMNS,
        "row_count": ROWS,
        "scrollback": null,
    })
}

fn encoded_bytes(value: &Value) -> usize {
    serde_json::to_vec(value).unwrap().len() + 4
}

fn hundredths_mib(bytes: usize) -> usize {
    bytes.saturating_mul(100).saturating_add(MIB / 2) / MIB
}

fn main() {
    let profiles = CellProfile::ALL.map(|profile| {
        let snapshot_bytes = encoded_bytes(&snapshot(profile));
        let update_bytes = encoded_bytes(&update(profile));
        json!({
            "cell_profile": profile.name(),
            "snapshot_bytes": snapshot_bytes,
            "snapshot_mib_hundredths": hundredths_mib(snapshot_bytes),
            "snapshot_fits_8_mib": snapshot_bytes <= 8 * MIB,
            "snapshot_fits_16_mib": snapshot_bytes <= 16 * MIB,
            "update_bytes": update_bytes,
            "update_mib_hundredths": hundredths_mib(update_bytes),
            "update_fits_8_mib": update_bytes <= 8 * MIB,
            "update_fits_16_mib": update_bytes <= 16 * MIB,
        })
    });
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "schema": "splinterm.protocol.grid-frame-envelope.v1",
            "columns": COLUMNS,
            "rows": ROWS,
            "snapshot_scrollback_rows": SNAPSHOT_SCROLLBACK_ROWS,
            "profiles": profiles,
        }))
        .unwrap()
    );
}
