//! Deterministic body-free comparison of one-buffer and legacy frame encoding.

use std::{hint::black_box, time::Instant};

use serde::Serialize;
use splinterm_protocol::{FrameEncodeError, MAX_FRAME_BYTES, encode_frame};

const WARMUPS: usize = 5;
const SAMPLES: usize = 20;
const ITERATIONS: usize = 40;

fn legacy_encode_frame<T: Serialize>(value: &T) -> Result<Vec<u8>, FrameEncodeError> {
    let body = serde_json::to_vec(value).map_err(FrameEncodeError::Serialize)?;
    if body.len() > MAX_FRAME_BYTES || body.len() > u32::MAX as usize {
        return Err(FrameEncodeError::TooLarge);
    }
    let length = u32::try_from(body.len()).map_err(|_| FrameEncodeError::TooLarge)?;
    let mut frame = Vec::with_capacity(body.len() + 4);
    frame.extend_from_slice(&length.to_be_bytes());
    frame.extend_from_slice(&body);
    Ok(frame)
}

fn payload() -> serde_json::Value {
    let rows = (0..80)
        .map(|row| {
            serde_json::json!({
                "index": row,
                "row": {
                    "row_id": row + 1,
                    "linebreak": false,
                    "cells": (0..80).map(|column| serde_json::json!({
                        "content": if column % 7 == 0 { "界" } else { "x" },
                        "spacer_remaining": serde_json::Value::Null,
                        "attributes": {
                            "bold": column % 11 == 0,
                            "italic": false,
                            "underline": "none",
                            "foreground_source": "default",
                            "foreground": 0xffffff,
                            "background_source": "default",
                            "background": 0
                        }
                    })).collect::<Vec<_>>()
                }
            })
        })
        .collect::<Vec<_>>();
    serde_json::json!({
        "type": "event",
        "subscription_id": 1,
        "sequence": 1,
        "event": {
            "type": "update",
            "update": {
                "base_revision": 100,
                "revision": 200,
                "rows": rows,
                "scrolls": [],
                "cursor": { "column": 79, "row": 23, "deferred_wrap": false },
                "title": null,
                "input_modes": null,
                "active_screen": null,
                "palette": null,
                "default_colors": null,
                "columns": null,
                "row_count": null,
                "scrollback": null
            }
        }
    })
}

fn measure(value: &serde_json::Value, candidate: bool) -> u64 {
    let started = Instant::now();
    for _ in 0..ITERATIONS {
        let frame = if candidate {
            encode_frame(black_box(value)).unwrap()
        } else {
            legacy_encode_frame(black_box(value)).unwrap()
        };
        black_box(frame);
    }
    u64::try_from(started.elapsed().as_nanos()).unwrap_or(u64::MAX)
}

fn summary(mut values: Vec<u64>) -> serde_json::Value {
    values.sort_unstable();
    let percentile = |numerator: usize| values[(values.len() - 1) * numerator / 100];
    serde_json::json!({
        "count": values.len(),
        "min_ns": values[0],
        "median_ns": percentile(50),
        "p95_ns": percentile(95),
        "max_ns": values[values.len() - 1],
    })
}

fn main() {
    assert!(!cfg!(debug_assertions), "benchmark requires --release");
    let value = payload();
    let legacy = legacy_encode_frame(&value).unwrap();
    let candidate = encode_frame(&value).unwrap();
    assert_eq!(candidate, legacy);

    for iteration in 0..WARMUPS {
        if iteration % 2 == 0 {
            black_box(measure(&value, false));
            black_box(measure(&value, true));
        } else {
            black_box(measure(&value, true));
            black_box(measure(&value, false));
        }
    }
    let mut control = Vec::with_capacity(SAMPLES);
    let mut candidate = Vec::with_capacity(SAMPLES);
    for iteration in 0..SAMPLES {
        if iteration % 2 == 0 {
            control.push(measure(&value, false));
            candidate.push(measure(&value, true));
        } else {
            candidate.push(measure(&value, true));
            control.push(measure(&value, false));
        }
    }
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "schema": "splinterm.performance.frame-encode.v1",
            "profile": "release",
            "warmups": WARMUPS,
            "samples": SAMPLES,
            "iterations_per_sample": ITERATIONS,
            "frame_bytes": legacy.len(),
            "byte_identical": true,
            "control": summary(control),
            "candidate": summary(candidate),
        }))
        .unwrap()
    );
}
