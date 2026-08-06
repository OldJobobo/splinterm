//! Non-graphical Plan 0022 terminal-state timing harness.
//!
//! Full comparative run:
//! `cargo run --release -p splinterm --example history-catchup-benchmark`
//!
//! Fast compile/execution check (small-update cases only):
//! `cargo run --release -p splinterm --example history-catchup-benchmark -- --smoke`

use std::{env, hint::black_box, time::Instant};

use serde_json::json;
use splinterm_core::SplintId;
use splinterm_protocol::{
    ActiveScreen, HistoryTransition, MouseTracking, TerminalInputModes, TerminalRow,
    TerminalRowPatch, TerminalScrollbackUpdate, TerminalSnapshot, TerminalUpdate,
};

#[allow(dead_code)]
#[path = "../src/wayland/terminal_state.rs"]
mod terminal_state;
#[allow(dead_code)]
#[path = "../src/viewport.rs"]
mod viewport;

use terminal_state::{MAX_CACHED_HISTORY_ROWS, apply_terminal_update, blank_row};
use viewport::ScrollbackViewport;

const COLUMNS: usize = 80;
const ROWS: usize = 24;
const ANSI_LINES: usize = 2_000;
const DEFAULT_WARMUPS: usize = 5;
const DEFAULT_SAMPLES: usize = 30;

#[derive(Clone, Copy)]
enum ViewportMode {
    Live,
    Detached,
}

#[derive(Clone, Copy)]
enum Activity {
    FocusedOnly,
    AllPanes,
    InactiveOnly,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum UpdateShape {
    Small,
    Ansi2000,
}

#[derive(Clone, Copy)]
struct Case {
    name: &'static str,
    history_rows: usize,
    viewport: ViewportMode,
    pane_count: usize,
    activity: Activity,
    shape: UpdateShape,
}

struct PaneState {
    snapshot: TerminalSnapshot,
    viewport: ScrollbackViewport,
}

const CASES: [Case; 14] = [
    case(
        "small-h0-live-p1-focused",
        0,
        ViewportMode::Live,
        1,
        Activity::FocusedOnly,
        UpdateShape::Small,
    ),
    case(
        "small-h1000-live-p1-focused",
        1_000,
        ViewportMode::Live,
        1,
        Activity::FocusedOnly,
        UpdateShape::Small,
    ),
    case(
        "small-h4096-live-p1-focused",
        4_096,
        ViewportMode::Live,
        1,
        Activity::FocusedOnly,
        UpdateShape::Small,
    ),
    case(
        "small-h4096-detached-p1-focused",
        4_096,
        ViewportMode::Detached,
        1,
        Activity::FocusedOnly,
        UpdateShape::Small,
    ),
    case(
        "small-h4096-live-p2-focused",
        4_096,
        ViewportMode::Live,
        2,
        Activity::FocusedOnly,
        UpdateShape::Small,
    ),
    case(
        "small-h4096-live-p4-focused",
        4_096,
        ViewportMode::Live,
        4,
        Activity::FocusedOnly,
        UpdateShape::Small,
    ),
    case(
        "small-h4096-live-p4-all",
        4_096,
        ViewportMode::Live,
        4,
        Activity::AllPanes,
        UpdateShape::Small,
    ),
    case(
        "small-h4096-live-p4-inactive",
        4_096,
        ViewportMode::Live,
        4,
        Activity::InactiveOnly,
        UpdateShape::Small,
    ),
    case(
        "ansi-h0-live-p1-focused",
        0,
        ViewportMode::Live,
        1,
        Activity::FocusedOnly,
        UpdateShape::Ansi2000,
    ),
    case(
        "ansi-h1000-live-p1-focused",
        1_000,
        ViewportMode::Live,
        1,
        Activity::FocusedOnly,
        UpdateShape::Ansi2000,
    ),
    case(
        "ansi-h4096-live-p1-focused",
        4_096,
        ViewportMode::Live,
        1,
        Activity::FocusedOnly,
        UpdateShape::Ansi2000,
    ),
    case(
        "ansi-h4096-detached-p1-focused",
        4_096,
        ViewportMode::Detached,
        1,
        Activity::FocusedOnly,
        UpdateShape::Ansi2000,
    ),
    case(
        "ansi-h4096-live-p4-all",
        4_096,
        ViewportMode::Live,
        4,
        Activity::AllPanes,
        UpdateShape::Ansi2000,
    ),
    case(
        "ansi-h4096-live-p4-inactive",
        4_096,
        ViewportMode::Live,
        4,
        Activity::InactiveOnly,
        UpdateShape::Ansi2000,
    ),
];

const fn case(
    name: &'static str,
    history_rows: usize,
    viewport: ViewportMode,
    pane_count: usize,
    activity: Activity,
    shape: UpdateShape,
) -> Case {
    Case {
        name,
        history_rows,
        viewport,
        pane_count,
        activity,
        shape,
    }
}

fn main() -> anyhow::Result<()> {
    let smoke = env::args().skip(1).any(|argument| argument == "--smoke");
    let warmups = if smoke { 0 } else { DEFAULT_WARMUPS };
    let samples = if smoke { 1 } else { DEFAULT_SAMPLES };
    let cases = CASES
        .iter()
        .copied()
        .filter(|case| !smoke || case.shape == UpdateShape::Small)
        .collect::<Vec<_>>();
    let mut durations = vec![Vec::<u64>::with_capacity(samples); cases.len()];

    for round in 0..warmups.saturating_add(samples) {
        let rotation = if cases.is_empty() {
            0
        } else {
            (round * 7) % cases.len()
        };
        for offset in 0..cases.len() {
            let index = (rotation + offset) % cases.len();
            let duration = measure_case(cases[index])?;
            if round >= warmups {
                durations[index].push(duration);
            }
        }
    }

    let records = cases
        .iter()
        .enumerate()
        .map(|(index, case)| {
            json!({
                "name": case.name,
                "history_rows": case.history_rows,
                "viewport": viewport_name(case.viewport),
                "pane_count": case.pane_count,
                "activity": activity_name(case.activity),
                "update_shape": shape_name(case.shape),
                "operation_updates": operation_updates(case.shape),
                "duration_ns": durations[index],
            })
        })
        .collect::<Vec<_>>();
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "schema": "splinterm.performance.history-catchup.v1",
            "clock": "std::time::Instant monotonic process clock",
            "build_profile": if cfg!(debug_assertions) { "debug" } else { "release" },
            "warmup_runs": warmups,
            "sample_runs": samples,
            "history_capacity_rows": MAX_CACHED_HISTORY_ROWS,
            "ansi_operation_lines": ANSI_LINES,
            "smoke": smoke,
            "cases": records,
        }))?
    );
    Ok(())
}

fn measure_case(case: Case) -> anyhow::Result<u64> {
    let mut panes = (0..case.pane_count)
        .map(|_| pane_state(case.history_rows, case.viewport))
        .collect::<Vec<_>>();
    let targets = activity_targets(case.activity, case.pane_count);
    let updates = operation_updates(case.shape);
    let mut prepared = targets
        .into_iter()
        .map(|index| {
            prepare_updates(&panes[index].snapshot, case.shape)
                .map(|updates| (index, updates.into_iter()))
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    let started = Instant::now();
    for _ in 0..updates {
        for (index, updates) in &mut prepared {
            let pane = &mut panes[*index];
            let previous_generation = pane.snapshot.history_generation;
            let previous_rows = history_rows_needed_for_viewport_transition(pane);
            let update = updates
                .next()
                .expect("the immutable update sequence matches the operation size");
            apply_terminal_update(&mut pane.snapshot, update)?;
            pane.viewport.observe_history_change(
                previous_generation,
                &previous_rows,
                &pane.snapshot,
            );
            black_box((pane.snapshot.revision, pane.viewport.offset_from_bottom()));
        }
    }
    Ok(u64::try_from(started.elapsed().as_nanos()).unwrap_or(u64::MAX))
}

fn history_rows_needed_for_viewport_transition(pane: &PaneState) -> Vec<TerminalRow> {
    if pane.viewport.is_live() {
        Vec::new()
    } else {
        pane.snapshot.scrollback_rows.clone()
    }
}

fn pane_state(history_rows: usize, mode: ViewportMode) -> PaneState {
    assert!(history_rows <= MAX_CACHED_HISTORY_ROWS);
    let history = (1..=u64::try_from(history_rows).unwrap())
        .map(history_row)
        .collect::<Vec<_>>();
    let visible_rows = (0..ROWS)
        .map(|index| {
            let mut row = blank_row(COLUMNS);
            row.row_id = Some(10_000_000 + u64::try_from(index).unwrap());
            row
        })
        .collect();
    let mut state = PaneState {
        snapshot: TerminalSnapshot {
            splint_id: SplintId::new(),
            incarnation: 1,
            revision: 1,
            columns: COLUMNS,
            rows: ROWS,
            cursor_column: 0,
            cursor_row: 0,
            cursor_deferred_wrap: false,
            active_screen: ActiveScreen::Normal,
            input_modes: normal_modes(),
            palette: vec![0; 256],
            default_colors: [0; 3],
            title: String::new(),
            visible_rows,
            history_generation: 1,
            oldest_available_scrollback_row_id: (history_rows > 0).then_some(1),
            newest_available_scrollback_row_id: (history_rows > 0)
                .then_some(u64::try_from(history_rows).unwrap()),
            scrollback_rows: history,
            available_scrollback_rows: history_rows,
            omitted_oldest_scrollback_rows: 0,
            images: None,
            exited_code: None,
            exited_signal: None,
        },
        viewport: ScrollbackViewport::default(),
    };
    if matches!(mode, ViewportMode::Detached) {
        state.viewport.scroll_up(ROWS / 2, &state.snapshot);
        assert!(!state.viewport.is_live());
    }
    state
}

fn history_row(id: u64) -> TerminalRow {
    let mut row = blank_row(1);
    row.row_id = Some(id);
    row.cells[0].content = "x".into();
    row
}

fn prepare_updates(
    snapshot: &TerminalSnapshot,
    shape: UpdateShape,
) -> anyhow::Result<Vec<TerminalUpdate>> {
    let mut projected = snapshot.clone();
    let mut updates = Vec::with_capacity(operation_updates(shape));
    for _ in 0..operation_updates(shape) {
        let update = match shape {
            UpdateShape::Small => small_update(&projected),
            UpdateShape::Ansi2000 => append_update(&projected),
        };
        apply_terminal_update(&mut projected, update.clone())?;
        updates.push(update);
    }
    Ok(updates)
}

fn small_update(snapshot: &TerminalSnapshot) -> TerminalUpdate {
    let mut row = snapshot.visible_rows[0].clone();
    row.cells[0].content = if snapshot.revision.is_multiple_of(2) {
        "a"
    } else {
        "b"
    }
    .into();
    base_update(snapshot, vec![TerminalRowPatch { index: 0, row }], None)
}

fn append_update(snapshot: &TerminalSnapshot) -> TerminalUpdate {
    let newest = snapshot.newest_available_scrollback_row_id.unwrap_or(0);
    let next = newest.saturating_add(1);
    let at_capacity = snapshot.available_scrollback_rows == MAX_CACHED_HISTORY_ROWS;
    let available_rows = if at_capacity {
        MAX_CACHED_HISTORY_ROWS
    } else {
        snapshot.available_scrollback_rows + 1
    };
    let oldest = if at_capacity {
        snapshot
            .oldest_available_scrollback_row_id
            .unwrap_or(1)
            .saturating_add(1)
    } else {
        1
    };
    base_update(
        snapshot,
        Vec::new(),
        Some(TerminalScrollbackUpdate {
            transition: HistoryTransition::Append {
                appended_rows: 1,
                trimmed_rows: usize::from(at_capacity),
            },
            history_generation: snapshot.history_generation,
            oldest_available_row_id: Some(oldest),
            newest_available_row_id: Some(next),
            rows: vec![history_row(next)],
            available_rows,
            omitted_oldest_rows: available_rows.saturating_sub(1),
        }),
    )
}

fn base_update(
    snapshot: &TerminalSnapshot,
    rows: Vec<TerminalRowPatch>,
    scrollback: Option<TerminalScrollbackUpdate>,
) -> TerminalUpdate {
    TerminalUpdate {
        base_revision: snapshot.revision,
        revision: snapshot.revision.saturating_add(1),
        rows,
        scrolls: Vec::new(),
        cursor: None,
        title: None,
        input_modes: None,
        active_screen: None,
        palette: None,
        default_colors: None,
        columns: None,
        row_count: None,
        scrollback,
        images: None,
    }
}

fn activity_targets(activity: Activity, pane_count: usize) -> Vec<usize> {
    match activity {
        Activity::FocusedOnly => vec![0],
        Activity::AllPanes => (0..pane_count).collect(),
        Activity::InactiveOnly => (1..pane_count).collect(),
    }
}

const fn operation_updates(shape: UpdateShape) -> usize {
    match shape {
        UpdateShape::Small => 1,
        UpdateShape::Ansi2000 => ANSI_LINES,
    }
}

const fn viewport_name(mode: ViewportMode) -> &'static str {
    match mode {
        ViewportMode::Live => "live",
        ViewportMode::Detached => "detached",
    }
}

const fn activity_name(activity: Activity) -> &'static str {
    match activity {
        Activity::FocusedOnly => "focused-only",
        Activity::AllPanes => "all-panes",
        Activity::InactiveOnly => "inactive-only",
    }
}

const fn shape_name(shape: UpdateShape) -> &'static str {
    match shape {
        UpdateShape::Small => "small",
        UpdateShape::Ansi2000 => "ansi-2000-lines",
    }
}

const fn normal_modes() -> TerminalInputModes {
    TerminalInputModes {
        application_cursor: false,
        application_keypad: false,
        focus_reporting: false,
        bracketed_paste: false,
        cursor_visible: true,
        cursor_blink: false,
        mouse_tracking: MouseTracking::None,
        sgr_mouse: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn case_matrix_covers_history_viewport_panes_shapes_and_activity() {
        assert!(CASES.iter().any(|case| case.history_rows == 0));
        assert!(CASES.iter().any(|case| case.history_rows == 1_000));
        assert!(CASES.iter().any(|case| case.history_rows == 4_096));
        assert!(
            CASES
                .iter()
                .any(|case| matches!(case.viewport, ViewportMode::Detached))
        );
        for pane_count in [1, 2, 4] {
            assert!(CASES.iter().any(|case| case.pane_count == pane_count));
        }
        assert!(CASES.iter().any(|case| case.shape == UpdateShape::Small));
        assert!(CASES.iter().any(|case| case.shape == UpdateShape::Ansi2000));
        assert!(
            CASES
                .iter()
                .any(|case| matches!(case.activity, Activity::InactiveOnly))
        );
    }

    #[test]
    fn append_update_preserves_the_exact_history_bound_and_viewport_contract() {
        let mut live = pane_state(MAX_CACHED_HISTORY_ROWS, ViewportMode::Live);
        assert!(history_rows_needed_for_viewport_transition(&live).is_empty());
        let update = append_update(&live.snapshot);
        let previous = live.snapshot.scrollback_rows.clone();
        apply_terminal_update(&mut live.snapshot, update).unwrap();
        live.viewport
            .observe_history_change(1, &previous, &live.snapshot);
        assert_eq!(live.snapshot.scrollback_rows.len(), MAX_CACHED_HISTORY_ROWS);
        assert_eq!(
            live.snapshot
                .scrollback_rows
                .first()
                .and_then(|row| row.row_id),
            Some(2)
        );
        assert_eq!(
            live.snapshot
                .scrollback_rows
                .last()
                .and_then(|row| row.row_id),
            Some(4_097)
        );
        assert!(live.viewport.is_live());

        let mut detached = pane_state(MAX_CACHED_HISTORY_ROWS, ViewportMode::Detached);
        assert_eq!(
            history_rows_needed_for_viewport_transition(&detached).len(),
            MAX_CACHED_HISTORY_ROWS
        );
        let anchor = detached.viewport.anchor_row_id();
        let previous = detached.snapshot.scrollback_rows.clone();
        let update = append_update(&detached.snapshot);
        apply_terminal_update(&mut detached.snapshot, update).unwrap();
        detached
            .viewport
            .observe_history_change(1, &previous, &detached.snapshot);
        assert!(!detached.viewport.is_live());
        assert_eq!(detached.viewport.anchor_row_id(), anchor);
        assert_eq!(detached.viewport.unseen_rows(), 1);

        let mut detached = pane_state(MAX_CACHED_HISTORY_ROWS, ViewportMode::Detached);
        let anchor = detached.viewport.anchor_row_id();
        for update in prepare_updates(&detached.snapshot, UpdateShape::Ansi2000).unwrap() {
            let previous = detached.snapshot.scrollback_rows.clone();
            apply_terminal_update(&mut detached.snapshot, update).unwrap();
            detached
                .viewport
                .observe_history_change(1, &previous, &detached.snapshot);
        }
        assert_eq!(detached.viewport.anchor_row_id(), anchor);
        assert_eq!(detached.viewport.unseen_rows(), ANSI_LINES);
        assert_eq!(
            detached.snapshot.scrollback_rows.len(),
            MAX_CACHED_HISTORY_ROWS
        );
    }

    #[test]
    fn activity_targets_are_bounded_and_role_exact() {
        assert_eq!(activity_targets(Activity::FocusedOnly, 4), [0]);
        assert_eq!(activity_targets(Activity::AllPanes, 4), [0, 1, 2, 3]);
        assert_eq!(activity_targets(Activity::InactiveOnly, 4), [1, 2, 3]);
    }
}
