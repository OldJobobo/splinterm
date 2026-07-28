## Review and evidence
Keep one active-worktree writer. Use one measured hypothesis per implementation
commit or clearly separated diff slice. Record rejected experiments. Require a
fresh read-only review after daemon publication ownership changes and before
closure. Do not claim a slice complete without both recorded validation evidence
and recorded review.
## Stop-loss
Stop and reassess when:
- a candidate cannot name the measured retained class it reduces;
- two controlled experiments fail to improve that class;
- memory falls only by moving bytes between daemon, client, SHM, or an unmeasured
  mapping;
- resnapshot, full-reload, redraw, or wakeup frequency rises;
- output CPU, child-write responsiveness, input, resize, or idle regresses beyond
  the accepted control allowance;
- correctness requires weakening revision, history, or reconstruction semantics;
- limits must be reduced to show improvement; or
- beating Ghostty requires a major renderer/protocol rewrite.
## Completion record
The failed 2026-07-26 closure attempt, exact binary identities, process-class
attribution, overhead results, repeated-cycle evidence, validation logs, and
read-only graphical preflight are retained in the
[Plan 0011 closure artifact](../benchmarks/artifacts/2026-07-26-plan0011-closure/summary.md).
This link is evidence of a failed gate, not a completion record. The later
[final closure attempt](../benchmarks/artifacts/2026-07-27-plan0011-final/summary.md)
records corrected 15.52 MiB repeated-cycle evidence and exact provenance, but is
also not a completion record because the randomized graphical batch violated its
workspace-cardinality guard and comparative evidence was not run. The later
[corrected final artifact](../benchmarks/artifacts/2026-07-27-plan0011-scroll-bound-fix-3/summary.md)
completed a valid randomized control/candidate comparison and records the final
no-go: aggregate and client retention regressed, so the comparative-terminal
stage remained correctly blocked.
At closure append links to:
- clean control and candidate source/binary identities;
- per-process and per-memory-class attribution;
- accepted and rejected hypotheses;
- non-graphical repeated-cycle evidence;
- approved graphical control/candidate and comparative evidence;
- correctness, package, fuzz, and review evidence; and
- the remaining Foot gap and any deliberately deferred architectural work.
# Plan 0011 final no-go
**Decision: do not close Plan 0011 as release-ready and do not tag `beta1`.**
## Correctness
Oversized coalesced scroll batches now fall back to bounded final-state viewport patches. Oversized append history uses the existing bounded `HistoryTransition::Replace`; no protocol limits, DTOs, or wire types were widened. Focused daemon/client/protocol tests and the full serial workspace suite pass.
## Non-graphical evidence
The corrected five-cycle/120-second daemon workload retained 13.68 MiB RSS and 9.42 MiB private-anonymous memory with zero overflow. Slice 4 allocator diagnostics still justify no allocator-specific product reclamation or manual trim.
## Graphical evidence
The final smoke passed workspace 8 / DP-2 placement, no-focus, marker, identity, and cleanup guards. The randomized clean-HEAD comparison completed with two warmups and ten measured samples per variant.
| Metric | Control median | Candidate median | Decision |
|---|---:|---:|---|
| Aggregate retained RSS | 70.45 MiB | 78.05 MiB | candidate 10.78% worse |
| Marker latency | 396.13 ms | 615.86 ms | regression |
| CPU ticks | 19.0 | 76.5 | regression |
| Daemon retained RSS | 34.11 MiB | 21.17 MiB | improvement |
| Client retained RSS | 36.31 MiB | 56.84 MiB | regression |
The daemon optimization works, but it moves high-water pressure into the client through a large coalesced update. The required 40% aggregate improvement is not established. Foot/Kitty/Ghostty comparisons were therefore correctly skipped.
## Next architecture
A future plan may evaluate bounded intermediate compact checkpoints/publication batches so fast clients receive protocol-sized updates while delayed subscribers still retain at most one compact snapshot. That is an architectural continuation, not a closure or Slice 4 reclamation tweak.
Exact source provenance, candidate/control binary hashes, raw records, process attribution, and serial validation are retained here.
{
  "blocker": "candidate aggregate retained growth, client retained growth, CPU, and marker latency regress against clean-HEAD control",
  "candidate_reduction_percent": -10.78458552813973,
  "closure_complete": false,
  "comparative_terminals_run": false,
  "decision": "no-go",
  "graphical_matrix_valid": true,
  "graphical_smoke_valid": true,
  "improvement_established": false,
  "next_architecture": "bounded intermediate compact checkpoints/publication batches that preserve protocol-sized client updates without restoring delayed-subscriber snapshot retention",
  "process_attribution": {
    "candidate": {
      "python": {
        "max_mib": 0.0,
        "median_mib": 0.0,
        "min_mib": 0.0,
        "samples": 10
      },
      "splinterd": {
        "max_mib": 21.45703125,
        "median_mib": 21.171875,
        "min_mib": 20.40625,
        "samples": 10
      },
      "splinterm": {
        "max_mib": 59.5859375,
        "median_mib": 56.8359375,
        "min_mib": 52.41796875,
        "samples": 10
      }
    },
    "control": {
      "python": {
        "max_mib": 0.0,
        "median_mib": 0.0,
        "min_mib": 0.0,
        "samples": 10
      },
      "splinterd": {
        "max_mib": 34.30078125,
        "median_mib": 34.109375,
        "min_mib": 33.9140625,
        "samples": 10
      },
      "splinterm": {
        "max_mib": 36.4296875,
        "median_mib": 36.310546875,
        "min_mib": 36.11328125,
        "samples": 10
      }
    }
  },
  "samples_per_variant": 10,
  "schema": "splinterm.plan0011.final-no-go.v1",
  "slice4_product_change_justified": false,
  "slices_1_to_3_implementation_valid": true,
  "tag_allowed": false
}
{
  "binaries": {
    "candidate_splinterd": {
      "path": "/tmp/splinterm-plan0011-final-candidate-bin/splinterd",
      "sha256": "e66e939de467f7143c3695b231734f0ed86f2a40804bd86f00ec7dbf58586b79",
      "size_bytes": 5125496
    },
    "candidate_splinterm": {
      "path": "/tmp/splinterm-plan0011-final-candidate-bin/splinterm",
      "sha256": "55879f8085797fc22ec3b4ef9911ec0b8fef5a94d92da8a701410dae422bb1a7",
      "size_bytes": 7662296
    },
    "candidate_splinterm-pty-child": {
      "path": "/tmp/splinterm-plan0011-final-candidate-bin/splinterm-pty-child",
      "sha256": "90df4a966566f6c084328090aa6d1d795f78aa7d3cc483e0d6ef6dd00eec9f77",
      "size_bytes": 490200
    },
    "control_splinterd": {
      "path": "/tmp/splinterm-plan0011-final-control-target/release/splinterd",
      "sha256": "7ff18e4adcb6e80316b1482e683c72322c2b2d630ade27d95c40c4bddcd37ca0",
      "size_bytes": 5095696
    },
    "control_splinterm": {
      "path": "/tmp/splinterm-plan0011-final-control-target/release/splinterm",
      "sha256": "55879f8085797fc22ec3b4ef9911ec0b8fef5a94d92da8a701410dae422bb1a7",
      "size_bytes": 7662296
    },
    "control_splinterm-pty-child": {
      "path": "/tmp/splinterm-plan0011-final-control-target/release/splinterm-pty-child",
      "sha256": "9d3225b2dc0af4d8ca2132a2caa1fdfe9d5fce8ef61351ef40c5abde6c96f6c7",
      "size_bytes": 490184
    }
  },
  "head": "0ce4fc62ed7ade2138fd35f492075942b415a571",
  "schema": "splinterm.plan0011.no-go-provenance.v1",
  "tracked_patch": {
    "path": "docs/benchmarks/artifacts/2026-07-27-plan0011-scroll-bound-fix-3/provenance/candidate-tracked.patch",
    "sha256": "ef4db184f8c64cfde545386cc1409a6e47b519c6221099fafac9a3fb7bce5540",
    "size_bytes": 182467
  },
  "untracked_source_bundle": {
    "path": "docs/benchmarks/artifacts/2026-07-27-plan0011-scroll-bound-fix-3/provenance/untracked-source.tar",
    "sha256": "c4101929edca15514bd4f2162253e5a5632b1e1cb02da54e6757b7a415329fb4",
    "size_bytes": 61440
  }
}
bash: git diff -- crates/splinterd/src/live.rs crates/splinterd...
grep: crates/splinterd/src/live.rs
grep: crates/splinterd/src/main.rs
grep: crates/splinterm/src/wayland.rs
bash: find docs/benchmarks/artifacts/2026-07-27-plan0011-scroll...
1:diff --git a/crates/splinterd/src/live.rs b/crates/splinterd/src/live.rs
5:@@ -4,8 +4,8 @@ use std::{
16:@@ -17,14 +17,14 @@ use splinterm_pty::{
35:@@ -150,6 +150,702 @@ pub enum LiveEvent {
118:+    scrollback_rows: Vec<CompactLiveRow>,
129:+        self.metadata.scrollback_rows = self
130:+            .scrollback_rows
204:+    let mut saw_scrollback = false;
207:+        let mut update_scrollback = false;
223:+                TerminalDamage::Scrollback => update_scrollback = true,
233:+        if update_scrollback {
237:+            saw_scrollback = true;
243:+    if saw_scrollback {
559:+    // idempotent ownership state eagerly while leaving event Drop as fallback.
738:@@ -199,6 +895,213 @@ impl Subscription {
952:@@ -283,6 +1186,8 @@ enum Command {
961:@@ -291,6 +1196,15 @@ pub struct LiveRuntimeMetrics {
977:@@ -300,6 +1214,34 @@ pub struct LiveRuntimeMetrics {
1012:@@ -307,6 +1249,10 @@ struct RuntimeMetrics {
1023:@@ -316,6 +1262,26 @@ struct RuntimeMetrics {
1050:@@ -323,6 +1289,66 @@ impl RuntimeMetrics {
1117:@@ -332,6 +1358,16 @@ impl RuntimeMetrics {
1134:@@ -341,6 +1377,58 @@ impl RuntimeMetrics {
1193:@@ -472,6 +1560,31 @@ impl LiveSplintHandle {
1199:+    pub async fn attach_compact_with_scrollback(
1201:+        max_scrollback_rows: usize,
1205:+                max_scrollback_rows.min(self.default_snapshot_rows),
1225:@@ -535,6 +1648,27 @@ impl LiveSplintRuntime {
1253:@@ -573,6 +1707,7 @@ impl LiveSplintRuntime {
1261:@@ -582,6 +1717,7 @@ impl LiveSplintRuntime {
1269:@@ -616,6 +1752,7 @@ impl LiveSplintRuntime {
1277:@@ -642,13 +1779,85 @@ impl LiveSplintRuntime {
1364:@@ -754,6 +1963,7 @@ async fn run_actor(
1372:@@ -765,6 +1975,7 @@ async fn run_actor(
1380:@@ -792,7 +2003,8 @@ async fn run_actor_body(
1390:@@ -833,6 +2045,7 @@ async fn run_actor_body(
1398:@@ -870,6 +2083,7 @@ async fn run_actor_body(
1406:@@ -899,6 +2113,17 @@ async fn run_actor_body(
1424:@@ -936,6 +2161,7 @@ async fn run_actor_body(
1432:@@ -961,14 +2187,9 @@ async fn run_actor_body(
1449:@@ -1002,7 +2223,8 @@ fn handle_command(
1459:@@ -1040,6 +2262,7 @@ fn handle_command(
1467:@@ -1129,11 +2352,18 @@ fn handle_command(
1476:                 snapshot_rows: max_rows.min(config.max_scrollback_snapshot_rows),
1487:@@ -1167,17 +2397,126 @@ fn handle_command(
1496:                 snapshot_rows: max_rows.min(config.max_scrollback_snapshot_rows),
1539:+                snapshot_rows: max_rows.min(config.max_scrollback_snapshot_rows),
1571:+                max_rows.min(config.max_scrollback_snapshot_rows),
1596:+                snapshot_rows: max_rows.min(config.max_scrollback_snapshot_rows),
1615:@@ -1195,11 +2534,19 @@ fn publish_updates(
1626:+        max_scrollback_rows: 0,
1630:+    let terminal_history_generation = terminal_metadata.scrollback().history_generation;
1635:@@ -1207,7 +2554,7 @@ fn publish_updates(
1644:@@ -1217,33 +2564,94 @@ fn publish_updates(
1723:+                        && snapshot.metadata.scrollback.history_generation
1760:@@ -1289,8 +2697,10 @@ fn process_output(
1771:@@ -1328,6 +2738,7 @@ fn process_output(
1779:@@ -1402,19 +2813,62 @@ fn process_output(
1854:@@ -1453,6 +2907,15 @@ fn resolve_image_content(
1861:+            max_scrollback_rows: 0,
1863:+        .scrollback()
1870:@@ -1484,32 +2947,196 @@ fn owned_snapshot(
1871:         scrollback: snapshot.scrollback(),
1880:+        owned.visible_rows.len() + owned.scrollback_rows.len(),
1884:+            .chain(&owned.scrollback_rows)
1924:+        max_scrollback_rows: requested_rows,
1927:+    let scrollback_rows = if history_policy == CompactHistoryPolicy::NoHistory {
1931:+            .scrollback_rows()
1935:+    let rows = visible_rows.len() + scrollback_rows.len();
1938:+        .chain(&scrollback_rows)
1941:+    let mut scrollback = snapshot.scrollback();
1943:+        scrollback.returned_rows = 0;
1944:+        scrollback.omitted_oldest_rows = scrollback.available_rows;
1954:+        scroll_region: snapshot.scroll_region(),
1962:+        scrollback_rows: Vec::new(),
1963:+        scrollback,
1977:+        scrollback_rows,
2001:-                    u64::try_from(owned.visible_rows.len() + owned.scrollback_rows.len())
2008:-                        .chain(&owned.scrollback_rows)
2023:+        rows: u64::try_from(snapshot.visible_rows.len() + snapshot.scrollback_rows.len())
2030:+        .chain(&snapshot.scrollback_rows)
2082:@@ -1537,6 +3164,29 @@ fn owned_row(row: splinterm_terminal::RowSnapshot<'_>) -> LiveRow {
2112:@@ -1569,6 +3219,49 @@ mod tests {
2162:@@ -1576,9 +3269,10 @@ mod tests {
2174:@@ -1594,6 +3288,7 @@ mod tests {
2182:@@ -1604,6 +3299,7 @@ mod tests {
2190:@@ -1627,6 +3323,7 @@ mod tests {
2198:@@ -1660,6 +3357,7 @@ mod tests {
2206:@@ -1670,6 +3368,7 @@ mod tests {
2214:@@ -1704,6 +3403,7 @@ mod tests {
2222:@@ -1714,6 +3414,7 @@ mod tests {
2230:@@ -1726,6 +3427,7 @@ mod tests {
2238:@@ -1742,6 +3444,7 @@ mod tests {
2246:@@ -1762,12 +3465,14 @@ mod tests {
2262:@@ -1775,7 +3480,503 @@ mod tests {
2608:+            scrollback_lines: 1_000,
2678:+        assert!(snapshot.scrollback_rows.len() <= 1_000);
2767:@@ -1783,34 +3984,109 @@ mod tests {
2897:@@ -1835,6 +4111,7 @@ mod tests {
2905:@@ -1846,6 +4123,7 @@ mod tests {
2913:@@ -1863,6 +4141,7 @@ mod tests {
2921:@@ -1891,12 +4170,473 @@ mod tests {
2965:+            max_scrollback_rows: 0,
2970:+            panic!("full-height normal scroll must be a proven append");
2983:+        assert_eq!(partial.scrollback_rows.len(), appended.min(1_000));
2986:+                .scrollback_rows
2990:+            full.scrollback_rows[full.scrollback_rows.len() - partial.scrollback_rows.len()..]
2995:+        assert_eq!(partial.metadata.scrollback, full.metadata.scrollback);
3001:+            max_scrollback_rows: 0,
3015:+        assert!(metadata_only.scrollback_rows.is_empty());
3017:+            metadata_only.metadata.scrollback.available_rows,
3018:+            full.metadata.scrollback.available_rows
3020:+        assert_eq!(metadata_only.metadata.scrollback.returned_rows, 0);
3022:+            metadata_only.metadata.scrollback.omitted_oldest_rows,
3023:+            metadata_only.metadata.scrollback.available_rows
3026:+            metadata_only.metadata.scrollback.oldest_available_row_id,
3027:+            full.metadata.scrollback.oldest_available_row_id
3030:+            metadata_only.metadata.scrollback.newest_available_row_id,
3031:+            full.metadata.scrollback.newest_available_row_id
3044:+            max_scrollback_rows: 0,
3055:+            max_scrollback_rows: 0,
3066:+            max_scrollback_rows: 0,
3395:@@ -1956,23 +4696,31 @@ mod tests {
3430:@@ -2010,6 +4758,7 @@ mod tests {
3438:diff --git a/crates/splinterd/src/main.rs b/crates/splinterd/src/main.rs
3442:@@ -24,9 +24,9 @@ use consent::{GrantStore, PeerIdentity};
3455:@@ -40,15 +40,15 @@ use splinterm_protocol::{
3456:     ErrorCode, HistoryTransition, ImageTransferMode, MAX_COLUMNS, MAX_FRAME_BYTES,
3457:     MAX_IMAGE_BYTES_PER_DAEMON, MAX_INPUT_BYTES, MAX_ROWS, MAX_SCROLLBACK_PAGE_ROWS,
3458:     MAX_SEARCH_CURSOR_BYTES, MAX_SEARCH_QUERY_BYTES, MAX_SEARCH_RESULTS,
3459:-    MAX_SNAPSHOT_SCROLLBACK_ROWS, MAX_SUBSCRIPTIONS, MouseTracking as WireMouseTracking,
3468:+    MAX_SNAPSHOT_SCROLLBACK_ROWS, MAX_SUBSCRIPTIONS, MAX_UPDATE_SCROLLS,
3480:@@ -1470,7 +1470,7 @@ struct Handled {
3489:@@ -4234,7 +4234,7 @@ async fn handle_authorized_request(
3491:             let scrollback_rows = scrollback_rows.min(MAX_SNAPSHOT_SCROLLBACK_ROWS);
3493:-                .attach_with_scrollback(scrollback_rows)
3494:+                .attach_compact_with_scrollback(scrollback_rows)
3498:@@ -5098,51 +5098,9 @@ fn frame_within_policy_limit(frame: &ServerFrame, maximum: Option<usize>) -> boo
3551:@@ -5156,8 +5114,8 @@ fn spawn_subscription(
3562:@@ -5209,26 +5167,8 @@ fn spawn_subscription(
3574:-                        let revision = current_revision(&handle, access.scrollback_rows).await;
3590:@@ -5296,20 +5236,16 @@ fn spawn_subscription(
3621:@@ -5351,12 +5287,12 @@ fn subscription_update_event(
3638:@@ -5577,6 +5513,64 @@ fn visible_row_changed(
3642:+fn wire_scrollback_update(
3644:+    scrollback: splinterm_terminal::ScrollbackSnapshot,
3649:+    let transition = if scrollback.history_generation != previous_history.generation {
3652:+        } else if scrollback.available_rows == 0 {
3655:+            HistoryTransition::Replace
3657:+    } else if appended_rows > 0 && appended_rows <= usize::from(MAX_ROWS) {
3663:+                .saturating_sub(scrollback.available_rows),
3666:+        HistoryTransition::Replace
3670:+            appended_rows.min(MAX_SNAPSHOT_SCROLLBACK_ROWS)
3672:+        HistoryTransition::Clear | HistoryTransition::Reflow | HistoryTransition::Replace => {
3673:+            MAX_SNAPSHOT_SCROLLBACK_ROWS
3680:+        history_generation: scrollback.history_generation,
3681:+        oldest_available_row_id: scrollback.oldest_available_row_id,
3682:+        newest_available_row_id: scrollback.newest_available_row_id,
3683:+        omitted_oldest_rows: scrollback.available_rows.saturating_sub(rows.len()),
3684:+        available_rows: scrollback.available_rows,
3689:+fn bound_wire_scrolls(scrolls: &mut Vec<TerminalScroll>, damaged: &mut [bool]) {
3690:+    if scrolls.len() > MAX_UPDATE_SCROLLS {
3692:+        // coalesced burst can contain more scroll damage records than one wire update
3694:+        // widening the protocol limit or emitting a semantically incomplete prefix.
3695:+        scrolls.clear();
3703:@@ -5657,6 +5651,18 @@ fn wire_update(
3707:+    bound_wire_scrolls(&mut scrolls, &mut damaged);
3708:+    let wire_scrollback = if scrollback {
3709:+        Some(wire_scrollback_update(
3710:+            &snapshot.scrollback_rows,
3711:+            snapshot.scrollback,
3722:@@ -5686,49 +5692,7 @@ fn wire_update(
3726:-        scrollback: scrollback.then(|| {
3728:-                .scrollback_rows
3730:-                .saturating_sub(MAX_SNAPSHOT_SCROLLBACK_ROWS);
3731:-            let rows: Vec<_> = snapshot.scrollback_rows[first..]
3737:-                if snapshot.scrollback.history_generation != previous_history.generation {
3740:-                    } else if snapshot.scrollback.available_rows == 0 {
3743:-                        HistoryTransition::Replace
3751:-                            .saturating_sub(snapshot.scrollback.available_rows),
3754:-                    HistoryTransition::Replace
3758:-                history_generation: snapshot.scrollback.history_generation,
3759:-                oldest_available_row_id: snapshot.scrollback.oldest_available_row_id,
3760:-                newest_available_row_id: snapshot.scrollback.newest_available_row_id,
3762:-                    .scrollback
3765:-                available_rows: snapshot.scrollback.available_rows,
3769:+        scrollback: wire_scrollback,
3773:@@ -5986,6 +5950,157 @@ mod tests {
3821:+        let scrollback = splinterm_terminal::ScrollbackSnapshot {
3835:+        let update = wire_scrollback_update(
3837:+            scrollback,
3840:+            usize::from(MAX_ROWS) + 1,
3843:+        assert_eq!(update.transition, HistoryTransition::Replace);
3844:+        assert_eq!(update.rows.len(), MAX_SNAPSHOT_SCROLLBACK_ROWS);
3850:+            scrolls: Vec::new(),
3859:+            scrollback: Some(update),
3866:+    fn oversized_scroll_batch_falls_back_to_bounded_viewport_patches() {
3867:+        let scroll = TerminalScroll {
3873:+        let mut scrolls = vec![scroll; MAX_UPDATE_SCROLLS + 1];
3874:+        let mut damaged = vec![false; MAX_ROWS as usize];
3876:+        bound_wire_scrolls(&mut scrolls, &mut damaged);
3878:+        assert!(scrolls.is_empty());
3880:+        assert!(damaged.len() <= splinterm_protocol::MAX_UPDATE_ROW_PATCHES);
3897:+        let scrollback = splinterm_terminal::ScrollbackSnapshot {
3910:+        let full = wire_scrollback_update(&full_rows, scrollback, previous, false, 2);
3911:+        let delta = wire_scrollback_update(&delta_rows, scrollback, previous, false, 2);
3931:diff --git a/crates/splinterm/src/wayland.rs b/crates/splinterm/src/wayland.rs
3935:@@ -7672,6 +7672,63 @@ mod tests {
3936:         assert_eq!(current.omitted_oldest_scrollback_rows, 0);
3945:+        current.scrollback_rows = (1..=4).map(|id| history_row(id, 0)).collect();
3946:+        current.available_scrollback_rows = 4;
3947:+        current.oldest_available_scrollback_row_id = Some(1);
3948:+        current.newest_available_scrollback_row_id = Some(4);
3964:+                rows: vec![splinterm_protocol::TerminalRowPatch {
3968:+                scrolls: Vec::new(),
3977:+                scrollback: Some(splinterm_protocol::TerminalScrollbackUpdate {
3978:+                    transition: HistoryTransition::Replace,
3993:+        assert_eq!(current.newest_available_scrollback_row_id, Some(20));
live.rs-17- };
live.rs-18- use splinterm_terminal::{
live.rs-19-     ActiveScreen, CellAttributesSnapshot, CellSnapshotContent, CursorSnapshot, Dimensions,
live.rs:20:     ImageContent, ImageContentId, ImageContentMetadata, ImagePlacement, ScrollDirection,
live.rs-21-     ScrollRegion, ScrollbackSnapshot, SearchPage, SnapshotRequest, Terminal, TerminalConfig,
live.rs-22-     TerminalDamage, TerminalEvent, TerminalModes, TerminalRevision, TerminalUpdate,
live.rs-23- };
live.rs-18- use splinterm_terminal::{
live.rs-19-     ActiveScreen, CellAttributesSnapshot, CellSnapshotContent, CursorSnapshot, Dimensions,
live.rs-20-     ImageContent, ImageContentId, ImageContentMetadata, ImagePlacement, ScrollDirection,
live.rs:21:     ScrollRegion, ScrollbackSnapshot, SearchPage, SnapshotRequest, Terminal, TerminalConfig,
live.rs-22-     TerminalDamage, TerminalEvent, TerminalModes, TerminalRevision, TerminalUpdate,
live.rs-23- };
live.rs-24- use thiserror::Error;
live.rs-105-     pub active_screen: ActiveScreen,
live.rs-106-     pub cursor: CursorSnapshot,
live.rs-107-     pub modes: TerminalModes,
live.rs:108:     pub scroll_region: ScrollRegion,
live.rs-109-     pub view_follows_live: bool,
live.rs-110-     pub title: String,
live.rs-111-     pub palette: [u32; 256],
live.rs-113-     pub image_contents: Vec<ImageContentMetadata>,
live.rs-114-     pub image_placements: Vec<ImagePlacement>,
live.rs-115-     pub visible_rows: Vec<LiveRow>,
live.rs:116:     pub scrollback_rows: Vec<LiveRow>,
live.rs-117-     pub scrollback: ScrollbackSnapshot,
live.rs-118-     pub exited: Option<ProcessExit>,
live.rs-119- }
live.rs-114-     pub image_placements: Vec<ImagePlacement>,
live.rs-115-     pub visible_rows: Vec<LiveRow>,
live.rs-116-     pub scrollback_rows: Vec<LiveRow>,
live.rs:117:     pub scrollback: ScrollbackSnapshot,
live.rs-118-     pub exited: Option<ProcessExit>,
live.rs-119- }
live.rs-120- 
live.rs-119- }
live.rs-120- 
live.rs-121- #[derive(Clone, Debug, Eq, PartialEq)]
live.rs:122: pub struct LiveScrollbackPage {
live.rs-123-     pub terminal_revision: TerminalRevision,
live.rs-124-     pub history_generation: u64,
live.rs-125-     pub title: String,
live.rs-229- struct CompactLiveSnapshot {
live.rs-230-     metadata: LiveSnapshot,
live.rs-231-     visible_rows: Vec<CompactLiveRow>,
live.rs:232:     scrollback_rows: Vec<CompactLiveRow>,
live.rs-233-     history_policy: CompactHistoryPolicy,
live.rs-234- }
live.rs-235- 
live.rs-240-             .into_iter()
live.rs-241-             .map(CompactLiveRow::into_live)
live.rs-242-             .collect();
live.rs:243:         self.metadata.scrollback_rows = self
live.rs-244-             .scrollback_rows
live.rs-245-             .into_iter()
live.rs-246-             .map(CompactLiveRow::into_live)
live.rs-241-             .map(CompactLiveRow::into_live)
live.rs-242-             .collect();
live.rs-243-         self.metadata.scrollback_rows = self
live.rs:244:             .scrollback_rows
live.rs-245-             .into_iter()
live.rs-246-             .map(CompactLiveRow::into_live)
live.rs-247-             .collect();
live.rs-315-         return CompactHistoryPolicy::FullHistory;
live.rs-316-     }
live.rs-317-     let mut appended_rows = 0_usize;
live.rs:318:     let mut saw_scrollback = false;
live.rs-319-     for update in updates {
live.rs-320-         let mut update_appended = 0_usize;
live.rs-321-         let mut update_scrollback = false;
live.rs-318-     let mut saw_scrollback = false;
live.rs-319-     for update in updates {
live.rs-320-         let mut update_appended = 0_usize;
live.rs:321:         let mut update_scrollback = false;
live.rs-322-         for damage in update.damage() {
live.rs-323-             match damage {
live.rs-324-                 TerminalDamage::FullSnapshot | TerminalDamage::Dimensions => {
live.rs-324-                 TerminalDamage::FullSnapshot | TerminalDamage::Dimensions => {
live.rs-325-                     return CompactHistoryPolicy::FullHistory;
live.rs-326-                 }
live.rs:327:                 TerminalDamage::Scroll {
live.rs-328-                     direction: ScrollDirection::Forward,
live.rs-329-                     region,
live.rs-330-                     rows,
live.rs-325-                     return CompactHistoryPolicy::FullHistory;
live.rs-326-                 }
live.rs-327-                 TerminalDamage::Scroll {
live.rs:328:                     direction: ScrollDirection::Forward,
live.rs-329-                     region,
live.rs-330-                     rows,
live.rs-331-                 } if region.start() == 0
live.rs-333-                 {
live.rs-334-                     update_appended = update_appended.saturating_add(*rows);
live.rs-335-                 }
live.rs:336:                 TerminalDamage::Scroll { .. } => return CompactHistoryPolicy::FullHistory,
live.rs-337-                 TerminalDamage::Scrollback => update_scrollback = true,
live.rs-338-                 TerminalDamage::Rows { .. }
live.rs-339-                 | TerminalDamage::Cursor { .. }
live.rs-334-                     update_appended = update_appended.saturating_add(*rows);
live.rs-335-                 }
live.rs-336-                 TerminalDamage::Scroll { .. } => return CompactHistoryPolicy::FullHistory,
live.rs:337:                 TerminalDamage::Scrollback => update_scrollback = true,
live.rs-338-                 TerminalDamage::Rows { .. }
live.rs-339-                 | TerminalDamage::Cursor { .. }
live.rs-340-                 | TerminalDamage::Modes
live.rs-344-                 | TerminalDamage::Images { .. } => {}
live.rs-345-             }
live.rs-346-         }
live.rs:347:         if update_scrollback {
live.rs-348-             if update_appended == 0 {
live.rs-349-                 return CompactHistoryPolicy::FullHistory;
live.rs-350-             }
live.rs-348-             if update_appended == 0 {
live.rs-349-                 return CompactHistoryPolicy::FullHistory;
live.rs-350-             }
live.rs:351:             saw_scrollback = true;
live.rs-352-             appended_rows = appended_rows.saturating_add(update_appended);
live.rs-353-         } else if update_appended > 0 {
live.rs-354-             return CompactHistoryPolicy::FullHistory;
live.rs-354-             return CompactHistoryPolicy::FullHistory;
live.rs-355-         }
live.rs-356-     }
live.rs:357:     if saw_scrollback {
live.rs-358-         CompactHistoryPolicy::AppendTail(appended_rows)
live.rs-359-     } else {
live.rs-360-         CompactHistoryPolicy::NoHistory
live.rs-703-     metrics: &Arc<RuntimeMetrics>,
live.rs-704-     mut build_snapshot: impl FnMut(CompactHistoryPolicy) -> CompactLiveSnapshot,
live.rs-705- ) -> CompactPublishOutcome {
live.rs:706:     // Inspect the bounded semantic tail without holding the mailbox across the
live.rs-707-     // expensive full-snapshot build. The actor is the only producer, while the
live.rs-708-     // receiver may consume and advance `generation` during construction.
live.rs-709-     let (observed_generation, had_pending, combined_policy) = {
live.rs-783-         }
live.rs-784-     } else if current.pending.is_some() {
live.rs-785-         // Only the receiver may advance the generation. It removes the complete
live.rs:786:         // tail, so finding a replacement tail here would make ordering unclear.
live.rs-787-         current.generation = current.generation.wrapping_add(1);
live.rs-788-         drop(current.pending.take());
live.rs-789-         drop(current.snapshot.take());
live.rs-1113-     pub reply_byte_limit: usize,
live.rs-1114-     pub subscriber_capacity: usize,
live.rs-1115-     pub max_subscribers: usize,
live.rs:1116:     pub max_scrollback_snapshot_rows: usize,
live.rs-1117-     pub exit_drain_timeout: Duration,
live.rs-1118-     pub hangup_grace: Duration,
live.rs-1119-     pub terminate_grace: Duration,
live.rs-1134-             reply_byte_limit: 64 * 1024,
live.rs-1135-             subscriber_capacity: 64,
live.rs-1136-             max_subscribers: 8,
live.rs:1137:             max_scrollback_snapshot_rows: 1_000,
live.rs-1138-             exit_drain_timeout: Duration::from_millis(250),
live.rs-1139-             hangup_grace: Duration::from_secs(30),
live.rs-1140-             terminate_grace: Duration::from_secs(30),
live.rs-1182-     Resize(PtySize, Reply<()>),
live.rs-1183-     Snapshot(usize, Reply<LiveSnapshot>),
live.rs-1184-     ImageContent(ImageContentId, u64, [u8; 32], Reply<ImageContent>),
live.rs:1185:     ScrollbackPage(Option<u64>, usize, Reply<LiveScrollbackPage>),
live.rs-1186-     Search(String, bool, usize, usize, Duration, Reply<LiveSearchPage>),
live.rs-1187-     Subscribe(usize, usize, Reply<Subscription>),
live.rs-1188-     Attach(usize, usize, Reply<(LiveSnapshot, Subscription)>),
live.rs-1462-     }
live.rs-1463- 
live.rs-1464-     pub async fn snapshot(&self) -> Result<LiveSnapshot, LiveError> {
live.rs:1465:         self.snapshot_with_scrollback(self.default_snapshot_rows)
live.rs-1466-             .await
live.rs-1467-     }
live.rs-1468- 
live.rs-1466-             .await
live.rs-1467-     }
live.rs-1468- 
live.rs:1469:     pub async fn snapshot_with_scrollback(
live.rs-1470-         &self,
live.rs-1471-         max_scrollback_rows: usize,
live.rs-1472-     ) -> Result<LiveSnapshot, LiveError> {
live.rs-1468- 
live.rs-1469-     pub async fn snapshot_with_scrollback(
live.rs-1470-         &self,
live.rs:1471:         max_scrollback_rows: usize,
live.rs-1472-     ) -> Result<LiveSnapshot, LiveError> {
live.rs-1473-         self.request(|reply| Command::Snapshot(max_scrollback_rows, reply))
live.rs-1474-             .await
live.rs-1470-         &self,
live.rs-1471-         max_scrollback_rows: usize,
live.rs-1472-     ) -> Result<LiveSnapshot, LiveError> {
live.rs:1473:         self.request(|reply| Command::Snapshot(max_scrollback_rows, reply))
live.rs-1474-             .await
live.rs-1475-     }
live.rs-1476- 
live.rs-1484-             .await
live.rs-1485-     }
live.rs-1486- 
live.rs:1487:     pub async fn scrollback_page(
live.rs-1488-         &self,
live.rs-1489-         before_row_id: u64,
live.rs-1490-         max_rows: usize,
live.rs-1488-         &self,
live.rs-1489-         before_row_id: u64,
live.rs-1490-         max_rows: usize,
live.rs:1491:     ) -> Result<LiveScrollbackPage, LiveError> {
live.rs-1492-         self.request(|reply| Command::ScrollbackPage(Some(before_row_id), max_rows, reply))
live.rs-1493-             .await
live.rs-1494-     }
live.rs-1489-         before_row_id: u64,
live.rs-1490-         max_rows: usize,
live.rs-1491-     ) -> Result<LiveScrollbackPage, LiveError> {
live.rs:1492:         self.request(|reply| Command::ScrollbackPage(Some(before_row_id), max_rows, reply))
live.rs-1493-             .await
live.rs-1494-     }
live.rs-1495- 
live.rs-1493-             .await
live.rs-1494-     }
live.rs-1495- 
live.rs:1496:     pub async fn start_scrollback_page(
live.rs-1497-         &self,
live.rs-1498-         max_rows: usize,
live.rs-1499-     ) -> Result<LiveScrollbackPage, LiveError> {
live.rs-1496-     pub async fn start_scrollback_page(
live.rs-1497-         &self,
live.rs-1498-         max_rows: usize,
live.rs:1499:     ) -> Result<LiveScrollbackPage, LiveError> {
live.rs-1500-         self.request(|reply| Command::ScrollbackPage(None, max_rows, reply))
live.rs-1501-             .await
live.rs-1502-     }
live.rs-1497-         &self,
live.rs-1498-         max_rows: usize,
live.rs-1499-     ) -> Result<LiveScrollbackPage, LiveError> {
live.rs:1500:         self.request(|reply| Command::ScrollbackPage(None, max_rows, reply))
live.rs-1501-             .await
live.rs-1502-     }
live.rs-1503- 
live.rs-1533-         .await
live.rs-1534-     }
live.rs-1535- 
live.rs:1536:     pub async fn attach_with_scrollback(
live.rs-1537-         &self,
live.rs-1538-         max_scrollback_rows: usize,
live.rs-1539-     ) -> Result<(LiveSnapshot, Subscription), LiveError> {
live.rs-1535- 
live.rs-1536-     pub async fn attach_with_scrollback(
live.rs-1537-         &self,
live.rs:1538:         max_scrollback_rows: usize,
live.rs-1539-     ) -> Result<(LiveSnapshot, Subscription), LiveError> {
live.rs-1540-         self.request(|reply| {
live.rs-1541-             Command::Attach(
live.rs-1539-     ) -> Result<(LiveSnapshot, Subscription), LiveError> {
live.rs-1540-         self.request(|reply| {
live.rs-1541-             Command::Attach(
live.rs:1542:                 max_scrollback_rows.min(self.default_snapshot_rows),
live.rs-1543-                 self.default_subscriber_capacity,
live.rs-1544-                 reply,
live.rs-1545-             )
live.rs-1562- 
live.rs-1563-     /// Attaches through the additive compact publication path used by the
live.rs-1564-     /// first-party daemon.
live.rs:1565:     pub async fn attach_compact_with_scrollback(
live.rs-1566-         &self,
live.rs-1567-         max_scrollback_rows: usize,
live.rs-1568-     ) -> Result<(LiveSnapshot, CompactSubscription), LiveError> {
live.rs-1564-     /// first-party daemon.
live.rs-1565-     pub async fn attach_compact_with_scrollback(
live.rs-1566-         &self,
live.rs:1567:         max_scrollback_rows: usize,
live.rs-1568-     ) -> Result<(LiveSnapshot, CompactSubscription), LiveError> {
live.rs-1569-         self.request(|reply| {
live.rs-1570-             Command::AttachCompact(
live.rs-1568-     ) -> Result<(LiveSnapshot, CompactSubscription), LiveError> {
live.rs-1569-         self.request(|reply| {
live.rs-1570-             Command::AttachCompact(
live.rs:1571:                 max_scrollback_rows.min(self.default_snapshot_rows),
live.rs-1572-                 self.default_subscriber_capacity,
live.rs-1573-                 reply,
live.rs-1574-             )
live.rs-1738-             splint_id,
live.rs-1739-             incarnation,
live.rs-1740-             commands: sender,
live.rs:1741:             default_snapshot_rows: config.max_scrollback_snapshot_rows,
live.rs-1742-             default_subscriber_capacity: config.subscriber_capacity,
live.rs-1743-             max_input_message_bytes: config.input_byte_limit / config.command_capacity.max(1),
live.rs-1744-             metrics: Arc::clone(&metrics),
live.rs-1826-         if let SubscriberEvents::Compact { snapshot_slot, .. } = &self.events {
live.rs-1827-             snapshot_slot.clear();
live.rs-1828-         }
live.rs:1829:         self.resnapshot.send_replace(true);
live.rs-1830-     }
live.rs-1831- }
live.rs-1832- 
live.rs-1985-         None
live.rs-1986-     };
live.rs-1987-     if let Some(status) = result.as_ref().ok().copied().or(forced_status) {
live.rs:1988:         exit_sender.send_replace(Some(status));
live.rs-1989-     }
live.rs-1990-     result
live.rs-1991- }
live.rs-2119-                                 })
live.rs-2120-                             {
live.rs-2121-                                 // One PTY read can synchronously publish exactly the
live.rs:2122:                                 // complete bounded semantic tail. Yield once before
live.rs-2123-                                 // another readable turn so an already-woken compact
live.rs-2124-                                 // consumer can take that tail without a false overflow.
live.rs-2125-                                 tokio::task::yield_now().await;
live.rs-2273-                 splint_id,
live.rs-2274-                 incarnation,
live.rs-2275-                 terminal,
live.rs:2276:                 max_rows.min(config.max_scrollback_snapshot_rows),
live.rs-2277-                 child_exit,
live.rs-2278-             );
live.rs-2279-             metrics.snapshot_builds.fetch_add(1, Ordering::Relaxed);
live.rs-2288-                 terminal, content_id, generation, digest,
live.rs-2289-             ));
live.rs-2290-         }
live.rs:2291:         Command::ScrollbackPage(before_row_id, max_rows, reply) => {
live.rs-2292-             let snapshot = terminal.snapshot(SnapshotRequest {
live.rs-2293-                 max_scrollback_rows: 1,
live.rs-2294-             });
live.rs-2290-         }
live.rs-2291-         Command::ScrollbackPage(before_row_id, max_rows, reply) => {
live.rs-2292-             let snapshot = terminal.snapshot(SnapshotRequest {
live.rs:2293:                 max_scrollback_rows: 1,
live.rs-2294-             });
live.rs-2295-             let scrollback = snapshot.scrollback();
live.rs-2296-             let before_row_id = match (before_row_id, scrollback.newest_available_row_id) {
live.rs-2292-             let snapshot = terminal.snapshot(SnapshotRequest {
live.rs-2293-                 max_scrollback_rows: 1,
live.rs-2294-             });
live.rs:2295:             let scrollback = snapshot.scrollback();
live.rs-2296-             let before_row_id = match (before_row_id, scrollback.newest_available_row_id) {
live.rs-2297-                 (Some(before_row_id), _) => before_row_id,
live.rs-2298-                 (None, Some(newest_row_id)) => {
live.rs-2293-                 max_scrollback_rows: 1,
live.rs-2294-             });
live.rs-2295-             let scrollback = snapshot.scrollback();
live.rs:2296:             let before_row_id = match (before_row_id, scrollback.newest_available_row_id) {
live.rs-2297-                 (Some(before_row_id), _) => before_row_id,
live.rs-2298-                 (None, Some(newest_row_id)) => {
live.rs-2299-                     let Some(before_row_id) = newest_row_id.checked_add(1) else {
live.rs-2304-                 }
live.rs-2305-                 (None, None) => 1,
live.rs-2306-             };
live.rs:2307:             let page = terminal.scrollback_page(
live.rs-2308-                 before_row_id,
live.rs-2309-                 max_rows.min(config.max_scrollback_snapshot_rows),
live.rs-2310-             );
live.rs-2306-             };
live.rs-2307-             let page = terminal.scrollback_page(
live.rs-2308-                 before_row_id,
live.rs:2309:                 max_rows.min(config.max_scrollback_snapshot_rows),
live.rs-2310-             );
live.rs-2311-             let _ = reply.send(Ok(LiveScrollbackPage {
live.rs-2312-                 terminal_revision: page.terminal_revision,
live.rs-2308-                 before_row_id,
live.rs-2309-                 max_rows.min(config.max_scrollback_snapshot_rows),
live.rs-2310-             );
live.rs:2311:             let _ = reply.send(Ok(LiveScrollbackPage {
live.rs-2312-                 terminal_revision: page.terminal_revision,
live.rs-2313-                 history_generation: page.history_generation,
live.rs-2314-                 title: snapshot.title().to_owned(),
live.rs-2312-                 terminal_revision: page.terminal_revision,
live.rs-2313-                 history_generation: page.history_generation,
live.rs-2314-                 title: snapshot.title().to_owned(),
live.rs:2315:                 oldest_available_row_id: scrollback.oldest_available_row_id,
live.rs-2316-                 newest_available_row_id: scrollback.newest_available_row_id,
live.rs-2317-                 rows: page.rows.into_iter().map(owned_row).collect(),
live.rs-2318-                 has_older: page.has_older,
live.rs-2313-                 history_generation: page.history_generation,
live.rs-2314-                 title: snapshot.title().to_owned(),
live.rs-2315-                 oldest_available_row_id: scrollback.oldest_available_row_id,
live.rs:2316:                 newest_available_row_id: scrollback.newest_available_row_id,
live.rs-2317-                 rows: page.rows.into_iter().map(owned_row).collect(),
live.rs-2318-                 has_older: page.has_older,
live.rs-2319-             }));
live.rs-2320-         }
live.rs-2321-         Command::Search(query, case_sensitive, skip_rows, maximum_results, deadline, reply) => {
live.rs-2322-             let snapshot = terminal.snapshot(SnapshotRequest {
live.rs:2323:                 max_scrollback_rows: 0,
live.rs-2324-             });
live.rs-2325-             let title = snapshot.title().to_owned();
live.rs-2326-             let page = terminal.search_normal(
live.rs-2356-                 resnapshot,
live.rs-2357-                 published_revision: terminal.revision(),
live.rs-2358-                 published_history_generation: terminal_history_generation(terminal),
live.rs:2359:                 snapshot_rows: max_rows.min(config.max_scrollback_snapshot_rows),
live.rs-2360-             });
live.rs-2361-             if publication_memory_metrics {
live.rs-2362-                 RuntimeMetrics::observe_max(
live.rs-2386-                 splint_id,
live.rs-2387-                 incarnation,
live.rs-2388-                 terminal,
live.rs:2389:                 max_rows.min(config.max_scrollback_snapshot_rows),
live.rs-2390-                 child_exit,
live.rs-2391-             );
live.rs-2392-             metrics.snapshot_builds.fetch_add(1, Ordering::Relaxed);
live.rs-2401-                 resnapshot,
live.rs-2402-                 published_revision: terminal.revision(),
live.rs-2403-                 published_history_generation: terminal_history_generation(terminal),
live.rs:2404:                 snapshot_rows: max_rows.min(config.max_scrollback_snapshot_rows),
live.rs-2405-             });
live.rs-2406-             if publication_memory_metrics {
live.rs-2407-                 RuntimeMetrics::observe_max(
live.rs-2444-                 resnapshot,
live.rs-2445-                 published_revision: terminal.revision(),
live.rs-2446-                 published_history_generation: terminal_history_generation(terminal),
live.rs:2447:                 snapshot_rows: max_rows.min(config.max_scrollback_snapshot_rows),
live.rs-2448-             });
live.rs-2449-             if publication_memory_metrics {
live.rs-2450-                 RuntimeMetrics::observe_max(
live.rs-2476-                 splint_id,
live.rs-2477-                 incarnation,
live.rs-2478-                 terminal,
live.rs:2479:                 max_rows.min(config.max_scrollback_snapshot_rows),
live.rs-2480-                 child_exit,
live.rs-2481-             );
live.rs-2482-             metrics.snapshot_builds.fetch_add(1, Ordering::Relaxed);
live.rs-2501-                 resnapshot,
live.rs-2502-                 published_revision: terminal.revision(),
live.rs-2503-                 published_history_generation: terminal_history_generation(terminal),
live.rs:2504:                 snapshot_rows: max_rows.min(config.max_scrollback_snapshot_rows),
live.rs-2505-             });
live.rs-2506-             if publication_memory_metrics {
live.rs-2507-                 RuntimeMetrics::observe_max(
live.rs-2541-         .unwrap_or(0);
live.rs-2542-     publication.published_revision = terminal.revision();
live.rs-2543-     let terminal_metadata = terminal.snapshot(SnapshotRequest {
live.rs:2544:         max_scrollback_rows: 0,
live.rs-2545-     });
live.rs-2546-     let terminal_dimensions = terminal_metadata.dimensions();
live.rs-2547-     let terminal_active_screen = terminal_metadata.active_screen();
live.rs-2545-     });
live.rs-2546-     let terminal_dimensions = terminal_metadata.dimensions();
live.rs-2547-     let terminal_active_screen = terminal_metadata.active_screen();
live.rs:2548:     let terminal_history_generation = terminal_metadata.scrollback().history_generation;
live.rs-2549-     drop(terminal_metadata);
live.rs-2550- 
live.rs-2551-     let mut overflows = 0_usize;
live.rs-2578-                 let permit = match sender.try_reserve() {
live.rs-2579-                     Ok(permit) => permit,
live.rs-2580-                     Err(mpsc::error::TrySendError::Full(_)) => {
live.rs:2581:                         subscriber.resnapshot.send_replace(true);
live.rs-2582-                         overflows = overflows.saturating_add(1);
live.rs-2583-                         return false;
live.rs-2584-                     }
live.rs-2618-                         policy,
live.rs-2619-                     );
live.rs-2620-                     if policy != CompactHistoryPolicy::FullHistory
live.rs:2621:                         && snapshot.metadata.scrollback.history_generation
live.rs-2622-                             != previous_history_generation
live.rs-2623-                     {
live.rs-2624-                         compact_snapshot_with_history(
live.rs-2637-                 CompactPublishOutcome::Published => true,
live.rs-2638-                 CompactPublishOutcome::Full => {
live.rs-2639-                     snapshot_slot.clear();
live.rs:2640:                     subscriber.resnapshot.send_replace(true);
live.rs-2641-                     overflows = overflows.saturating_add(1);
live.rs-2642-                     false
live.rs-2643-                 }
live.rs-2825-                 Ok(permit) => permit,
live.rs-2826-                 Err(mpsc::error::TrySendError::Full(_)) => {
live.rs-2827-                     overflows = overflows.saturating_add(1);
live.rs:2828:                     subscriber.resnapshot.send_replace(true);
live.rs-2829-                     return false;
live.rs-2830-                 }
live.rs-2831-                 Err(mpsc::error::TrySendError::Closed(_)) => return false,
live.rs-2847-                 Err(mpsc::error::TrySendError::Full(_)) => {
live.rs-2848-                     overflows = overflows.saturating_add(1);
live.rs-2849-                     snapshot_slot.clear();
live.rs:2850:                     subscriber.resnapshot.send_replace(true);
live.rs-2851-                     return false;
live.rs-2852-                 }
live.rs-2853-                 Err(mpsc::error::TrySendError::Closed(_)) => {
live.rs-2910- fn terminal_history_generation(terminal: &Terminal) -> u64 {
live.rs-2911-     terminal
live.rs-2912-         .snapshot(SnapshotRequest {
live.rs:2913:             max_scrollback_rows: 0,
live.rs-2914-         })
live.rs-2915-         .scrollback()
live.rs-2916-         .history_generation
live.rs-2912-         .snapshot(SnapshotRequest {
live.rs-2913-             max_scrollback_rows: 0,
live.rs-2914-         })
live.rs:2915:         .scrollback()
live.rs-2916-         .history_generation
live.rs-2917- }
live.rs-2918- 
live.rs-2925- ) -> LiveSnapshot {
live.rs-2926-     let trace_started = perf_trace_enabled().then(Instant::now);
live.rs-2927-     let snapshot = terminal.snapshot(SnapshotRequest {
live.rs:2928:         max_scrollback_rows: max_rows,
live.rs-2929-     });
live.rs-2930-     let owned = LiveSnapshot {
live.rs-2931-         splint_id,
live.rs-2935-         active_screen: snapshot.active_screen(),
live.rs-2936-         cursor: snapshot.cursor(),
live.rs-2937-         modes: snapshot.modes(),
live.rs:2938:         scroll_region: snapshot.scroll_region(),
live.rs-2939-         view_follows_live: snapshot.view_follows_live(),
live.rs-2940-         title: snapshot.title().to_owned(),
live.rs-2941-         palette: *snapshot.palette(),
live.rs-2943-         image_contents: snapshot.image_contents().collect(),
live.rs-2944-         image_placements: snapshot.image_placements().collect(),
live.rs-2945-         visible_rows: snapshot.visible_rows().map(owned_row).collect(),
live.rs:2946:         scrollback_rows: snapshot.scrollback_rows().map(owned_row).collect(),
live.rs-2947-         scrollback: snapshot.scrollback(),
live.rs-2948-         exited,
live.rs-2949-     };
live.rs-2944-         image_placements: snapshot.image_placements().collect(),
live.rs-2945-         visible_rows: snapshot.visible_rows().map(owned_row).collect(),
live.rs-2946-         scrollback_rows: snapshot.scrollback_rows().map(owned_row).collect(),
live.rs:2947:         scrollback: snapshot.scrollback(),
live.rs-2948-         exited,
live.rs-2949-     };
live.rs-2950-     emit_owned_snapshot_trace(
live.rs-2952-         splint_id,
live.rs-2953-         incarnation,
live.rs-2954-         owned.revision,
live.rs:2955:         owned.visible_rows.len() + owned.scrollback_rows.len(),
live.rs-2956-         owned
live.rs-2957-             .visible_rows
live.rs-2958-             .iter()
live.rs-2956-         owned
live.rs-2957-             .visible_rows
live.rs-2958-             .iter()
live.rs:2959:             .chain(&owned.scrollback_rows)
live.rs-2960-             .map(|row| row.cells.len())
live.rs-2961-             .sum(),
live.rs-2962-     );
live.rs-2996-         CompactHistoryPolicy::AppendTail(rows) => rows.min(max_rows),
live.rs-2997-     };
live.rs-2998-     let snapshot = terminal.snapshot(SnapshotRequest {
live.rs:2999:         max_scrollback_rows: requested_rows,
live.rs-3000-     });
live.rs-3001-     let visible_rows = snapshot.visible_rows().map(compact_row).collect::<Vec<_>>();
live.rs-3002-     let scrollback_rows = if history_policy == CompactHistoryPolicy::NoHistory {
live.rs-2999-         max_scrollback_rows: requested_rows,
live.rs-3000-     });
live.rs-3001-     let visible_rows = snapshot.visible_rows().map(compact_row).collect::<Vec<_>>();
live.rs:3002:     let scrollback_rows = if history_policy == CompactHistoryPolicy::NoHistory {
live.rs-3003-         Vec::new()
live.rs-3004-     } else {
live.rs-3005-         snapshot
live.rs-3003-         Vec::new()
live.rs-3004-     } else {
live.rs-3005-         snapshot
live.rs:3006:             .scrollback_rows()
live.rs-3007-             .map(compact_row)
live.rs-3008-             .collect::<Vec<_>>()
live.rs-3009-     };
live.rs-3007-             .map(compact_row)
live.rs-3008-             .collect::<Vec<_>>()
live.rs-3009-     };
live.rs:3010:     let rows = visible_rows.len() + scrollback_rows.len();
live.rs-3011-     let cells = visible_rows
live.rs-3012-         .iter()
live.rs-3013-         .chain(&scrollback_rows)
live.rs-3010-     let rows = visible_rows.len() + scrollback_rows.len();
live.rs-3011-     let cells = visible_rows
live.rs-3012-         .iter()
live.rs:3013:         .chain(&scrollback_rows)
live.rs-3014-         .map(|row| row.cells.len())
live.rs-3015-         .sum();
live.rs-3016-     let mut scrollback = snapshot.scrollback();
live.rs-3013-         .chain(&scrollback_rows)
live.rs-3014-         .map(|row| row.cells.len())
live.rs-3015-         .sum();
live.rs:3016:     let mut scrollback = snapshot.scrollback();
live.rs-3017-     if history_policy == CompactHistoryPolicy::NoHistory {
live.rs-3018-         scrollback.returned_rows = 0;
live.rs-3019-         scrollback.omitted_oldest_rows = scrollback.available_rows;
live.rs-3015-         .sum();
live.rs-3016-     let mut scrollback = snapshot.scrollback();
live.rs-3017-     if history_policy == CompactHistoryPolicy::NoHistory {
live.rs:3018:         scrollback.returned_rows = 0;
live.rs-3019-         scrollback.omitted_oldest_rows = scrollback.available_rows;
live.rs-3020-     }
live.rs-3021-     let metadata = LiveSnapshot {
live.rs-3016-     let mut scrollback = snapshot.scrollback();
live.rs-3017-     if history_policy == CompactHistoryPolicy::NoHistory {
live.rs-3018-         scrollback.returned_rows = 0;
live.rs:3019:         scrollback.omitted_oldest_rows = scrollback.available_rows;
live.rs-3020-     }
live.rs-3021-     let metadata = LiveSnapshot {
live.rs-3022-         splint_id,
live.rs-3026-         active_screen: snapshot.active_screen(),
live.rs-3027-         cursor: snapshot.cursor(),
live.rs-3028-         modes: snapshot.modes(),
live.rs:3029:         scroll_region: snapshot.scroll_region(),
live.rs-3030-         view_follows_live: snapshot.view_follows_live(),
live.rs-3031-         title: snapshot.title().to_owned(),
live.rs-3032-         palette: *snapshot.palette(),
live.rs-3034-         image_contents: snapshot.image_contents().collect(),
live.rs-3035-         image_placements: snapshot.image_placements().collect(),
live.rs-3036-         visible_rows: Vec::new(),
live.rs:3037:         scrollback_rows: Vec::new(),
live.rs-3038-         scrollback,
live.rs-3039-         exited,
live.rs-3040-     };
live.rs-3035-         image_placements: snapshot.image_placements().collect(),
live.rs-3036-         visible_rows: Vec::new(),
live.rs-3037-         scrollback_rows: Vec::new(),
live.rs:3038:         scrollback,
live.rs-3039-         exited,
live.rs-3040-     };
live.rs-3041-     emit_owned_snapshot_trace(
live.rs-3049-     CompactLiveSnapshot {
live.rs-3050-         metadata,
live.rs-3051-         visible_rows,
live.rs:3052:         scrollback_rows,
live.rs-3053-         history_policy,
live.rs-3054-     }
live.rs-3055- }
live.rs-3081- 
live.rs-3082- fn compact_snapshot_attribution(snapshot: &CompactLiveSnapshot) -> SnapshotAttribution {
live.rs-3083-     let mut attribution = SnapshotAttribution {
live.rs:3084:         rows: u64::try_from(snapshot.visible_rows.len() + snapshot.scrollback_rows.len())
live.rs-3085-             .unwrap_or(u64::MAX),
live.rs-3086-         ..SnapshotAttribution::default()
live.rs-3087-     };
live.rs-3088-     for cell in snapshot
live.rs-3089-         .visible_rows
live.rs-3090-         .iter()
live.rs:3091:         .chain(&snapshot.scrollback_rows)
live.rs-3092-         .flat_map(|row| &row.cells)
live.rs-3093-     {
live.rs-3094-         attribution.cells = attribution.cells.saturating_add(1);
live.rs-3821-     async fn compact_slot_pairs_thousand_row_history_clear_and_reflow_revision() {
live.rs-3822-         let incarnation = ProcessIncarnation::allocate();
live.rs-3823-         let config = TerminalConfig {
live.rs:3824:             scrollback_lines: 1_000,
live.rs-3825-             ..TerminalConfig::default()
live.rs-3826-         };
live.rs-3827-         let mut terminal = Terminal::new(8, 2, config);
live.rs-3891-         assert_eq!(snapshot.revision, terminal.revision());
live.rs-3892-         assert_eq!(snapshot.dimensions.columns, 10);
live.rs-3893-         assert_eq!(snapshot.dimensions.rows, 3);
live.rs:3894:         assert!(snapshot.scrollback_rows.len() <= 1_000);
live.rs-3895-         assert!(snapshot_text(&snapshot).contains("after-clear"));
live.rs-3896-         assert_eq!(metrics.snapshot().queued_snapshot_events_high_water, 1);
live.rs-3897-     }
live.rs-3897-     }
live.rs-3898- 
live.rs-3899-     #[test]
live.rs:3900:     fn snapshot_slot_replacement_and_receiver_drop_release_exact_ownership() {
live.rs-3901-         let incarnation = ProcessIncarnation::allocate();
live.rs-3902-         let mut terminal = Terminal::new(8, 2, TerminalConfig::default());
live.rs-3903-         let metrics = Arc::new(RuntimeMetrics::default());
live.rs-4016-             &mut subscribers,
live.rs-4017-             &metrics,
live.rs-4018-         );
live.rs:4019:         subscribers[0].resnapshot.send_replace(true);
live.rs-4020-         resnapshot.changed().await.unwrap();
live.rs-4021-         let (received, trailing_exit) = subscription.recv_coalesced().await;
live.rs-4022-         assert!(matches!(received, SubscriptionReceive::ResnapshotRequired));
live.rs-4210-         terminal.advance(b"one\r\ntwo\r\nthree");
live.rs-4211-         let updates = terminal.updates_since(base).unwrap().into_updates();
live.rs-4212-         let metadata = terminal.snapshot(SnapshotRequest {
live.rs:4213:             max_scrollback_rows: 0,
live.rs-4214-         });
live.rs-4215-         let policy =
live.rs-4216-             compact_history_policy(&updates, metadata.dimensions(), metadata.active_screen());
live.rs-4215-         let policy =
live.rs-4216-             compact_history_policy(&updates, metadata.dimensions(), metadata.active_screen());
live.rs-4217-         let CompactHistoryPolicy::AppendTail(appended) = policy else {
live.rs:4218:             panic!("full-height normal scroll must be a proven append");
live.rs-4219-         };
live.rs-4220-         assert!(appended > 0);
live.rs-4221-         let partial =
live.rs-4228-             None,
live.rs-4229-             CompactHistoryPolicy::FullHistory,
live.rs-4230-         );
live.rs:4231:         assert_eq!(partial.scrollback_rows.len(), appended.min(1_000));
live.rs-4232-         assert_eq!(
live.rs-4233-             partial
live.rs-4234-                 .scrollback_rows
live.rs-4231-         assert_eq!(partial.scrollback_rows.len(), appended.min(1_000));
live.rs-4232-         assert_eq!(
live.rs-4233-             partial
live.rs:4234:                 .scrollback_rows
live.rs-4235-                 .iter()
live.rs-4236-                 .map(|row| row.row_id)
live.rs-4237-                 .collect::<Vec<_>>(),
live.rs-4235-                 .iter()
live.rs-4236-                 .map(|row| row.row_id)
live.rs-4237-                 .collect::<Vec<_>>(),
live.rs:4238:             full.scrollback_rows[full.scrollback_rows.len() - partial.scrollback_rows.len()..]
live.rs-4239-                 .iter()
live.rs-4240-                 .map(|row| row.row_id)
live.rs-4241-                 .collect::<Vec<_>>()
live.rs-4240-                 .map(|row| row.row_id)
live.rs-4241-                 .collect::<Vec<_>>()
live.rs-4242-         );
live.rs:4243:         assert_eq!(partial.metadata.scrollback, full.metadata.scrollback);
live.rs-4244- 
live.rs-4245-         let base = terminal.revision();
live.rs-4246-         terminal.advance(b"X");
live.rs-4246-         terminal.advance(b"X");
live.rs-4247-         let updates = terminal.updates_since(base).unwrap().into_updates();
live.rs-4248-         let metadata = terminal.snapshot(SnapshotRequest {
live.rs:4249:             max_scrollback_rows: 0,
live.rs-4250-         });
live.rs-4251-         assert_eq!(
live.rs-4252-             compact_history_policy(&updates, metadata.dimensions(), metadata.active_screen(),),
[100 matches limit reached. Use limit=200 for more, or refine pattern]
main.rs-24- use persistence::MetadataStore;
main.rs-25- use rustix::net::{SendAncillaryBuffer, SendAncillaryMessage, SendFlags, sendmsg};
main.rs-26- use splinterd::{
main.rs:27:     CompactSubscription, LiveError, LiveEvent, LiveScrollbackPage, LiveSearchPage, LiveSnapshot,
main.rs-28-     LiveSplintConfig, LiveSplintHandle, LiveSplintRuntime, ProcessIncarnation, SubscriptionReceive,
main.rs-29-     authorization, executable_identity,
main.rs-30-     image_transport::{TransferAdmission, TransferAdmissionError, sealed_image_memfd},
main.rs-37- use splinterm_protocol::{
main.rs-38-     AccessGrant, AccessScope, ActiveScreen as WireActiveScreen, CellAttributes, ClientFrame,
main.rs-39-     ClientRole, ColorSource, ControlStatus, ControlTransferDecision, ControlTransferOutcome,
main.rs:40:     ErrorCode, HistoryTransition, ImageTransferMode, MAX_COLUMNS, MAX_FRAME_BYTES,
main.rs-41-     MAX_IMAGE_BYTES_PER_DAEMON, MAX_INPUT_BYTES, MAX_ROWS, MAX_SCROLLBACK_PAGE_ROWS,
main.rs-42-     MAX_SEARCH_CURSOR_BYTES, MAX_SEARCH_QUERY_BYTES, MAX_SEARCH_RESULTS,
main.rs-43-     MAX_SNAPSHOT_SCROLLBACK_ROWS, MAX_SUBSCRIPTIONS, MAX_UPDATE_SCROLLS,
main.rs-38-     AccessGrant, AccessScope, ActiveScreen as WireActiveScreen, CellAttributes, ClientFrame,
main.rs-39-     ClientRole, ColorSource, ControlStatus, ControlTransferDecision, ControlTransferOutcome,
main.rs-40-     ErrorCode, HistoryTransition, ImageTransferMode, MAX_COLUMNS, MAX_FRAME_BYTES,
main.rs:41:     MAX_IMAGE_BYTES_PER_DAEMON, MAX_INPUT_BYTES, MAX_ROWS, MAX_SCROLLBACK_PAGE_ROWS,
main.rs-42-     MAX_SEARCH_CURSOR_BYTES, MAX_SEARCH_QUERY_BYTES, MAX_SEARCH_RESULTS,
main.rs-43-     MAX_SNAPSHOT_SCROLLBACK_ROWS, MAX_SUBSCRIPTIONS, MAX_UPDATE_SCROLLS,
main.rs-44-     MouseTracking as WireMouseTracking, PROTOCOL_VERSION, ProcessExitStatus, ProtocolError,
main.rs-39-     ClientRole, ColorSource, ControlStatus, ControlTransferDecision, ControlTransferOutcome,
main.rs-40-     ErrorCode, HistoryTransition, ImageTransferMode, MAX_COLUMNS, MAX_FRAME_BYTES,
main.rs-41-     MAX_IMAGE_BYTES_PER_DAEMON, MAX_INPUT_BYTES, MAX_ROWS, MAX_SCROLLBACK_PAGE_ROWS,
main.rs:42:     MAX_SEARCH_CURSOR_BYTES, MAX_SEARCH_QUERY_BYTES, MAX_SEARCH_RESULTS,
main.rs-43-     MAX_SNAPSHOT_SCROLLBACK_ROWS, MAX_SUBSCRIPTIONS, MAX_UPDATE_SCROLLS,
main.rs-44-     MouseTracking as WireMouseTracking, PROTOCOL_VERSION, ProcessExitStatus, ProtocolError,
main.rs-45-     Request, Response, RestoreLeafResult, ScrollDirection as WireScrollDirection,
main.rs-40-     ErrorCode, HistoryTransition, ImageTransferMode, MAX_COLUMNS, MAX_FRAME_BYTES,
main.rs-41-     MAX_IMAGE_BYTES_PER_DAEMON, MAX_INPUT_BYTES, MAX_ROWS, MAX_SCROLLBACK_PAGE_ROWS,
main.rs-42-     MAX_SEARCH_CURSOR_BYTES, MAX_SEARCH_QUERY_BYTES, MAX_SEARCH_RESULTS,
main.rs:43:     MAX_SNAPSHOT_SCROLLBACK_ROWS, MAX_SUBSCRIPTIONS, MAX_UPDATE_SCROLLS,
main.rs-44-     MouseTracking as WireMouseTracking, PROTOCOL_VERSION, ProcessExitStatus, ProtocolError,
main.rs-45-     Request, Response, RestoreLeafResult, ScrollDirection as WireScrollDirection,
main.rs-46-     ScrollbackPage as WireScrollbackPage, SearchMatch as WireSearchMatch,
main.rs-42-     MAX_SEARCH_CURSOR_BYTES, MAX_SEARCH_QUERY_BYTES, MAX_SEARCH_RESULTS,
main.rs-43-     MAX_SNAPSHOT_SCROLLBACK_ROWS, MAX_SUBSCRIPTIONS, MAX_UPDATE_SCROLLS,
main.rs-44-     MouseTracking as WireMouseTracking, PROTOCOL_VERSION, ProcessExitStatus, ProtocolError,
main.rs:45:     Request, Response, RestoreLeafResult, ScrollDirection as WireScrollDirection,
main.rs-46-     ScrollbackPage as WireScrollbackPage, SearchMatch as WireSearchMatch,
main.rs-47-     SearchPage as WireSearchPage, ServerFrame, ServerLimits, SplintLifecycle, SplintRuntimeSummary,
main.rs-48-     SubscriptionEvent, TerminalCell, TerminalCursor, TerminalInputModes, TerminalProvenance,
main.rs-43-     MAX_SNAPSHOT_SCROLLBACK_ROWS, MAX_SUBSCRIPTIONS, MAX_UPDATE_SCROLLS,
main.rs-44-     MouseTracking as WireMouseTracking, PROTOCOL_VERSION, ProcessExitStatus, ProtocolError,
main.rs-45-     Request, Response, RestoreLeafResult, ScrollDirection as WireScrollDirection,
main.rs:46:     ScrollbackPage as WireScrollbackPage, SearchMatch as WireSearchMatch,
main.rs-47-     SearchPage as WireSearchPage, ServerFrame, ServerLimits, SplintLifecycle, SplintRuntimeSummary,
main.rs-48-     SubscriptionEvent, TerminalCell, TerminalCursor, TerminalInputModes, TerminalProvenance,
main.rs-49-     TerminalRow, TerminalRowPatch, TerminalScroll, TerminalScrollbackUpdate, TerminalSnapshot,
main.rs-46-     ScrollbackPage as WireScrollbackPage, SearchMatch as WireSearchMatch,
main.rs-47-     SearchPage as WireSearchPage, ServerFrame, ServerLimits, SplintLifecycle, SplintRuntimeSummary,
main.rs-48-     SubscriptionEvent, TerminalCell, TerminalCursor, TerminalInputModes, TerminalProvenance,
main.rs:49:     TerminalRow, TerminalRowPatch, TerminalScroll, TerminalScrollbackUpdate, TerminalSnapshot,
main.rs-50-     TerminalUpdate as WireTerminalUpdate, TopologyChange, TopologyChangeKind, TopologySnapshot,
main.rs-51-     UnderlineStyle as WireUnderlineStyle, encode_frame, image_content_socket_path,
main.rs-52-     perf_trace::{PerfTraceEvent, emit_perf_trace, perf_trace_enabled},
main.rs-54- use splinterm_pty::{LinuxPtyBackend, PtyCommand, PtySize, default_shell};
main.rs-55- use splinterm_terminal::{
main.rs-56-     ActiveScreen, ColorSource as TerminalColorSource, DEFAULT_KITTY_UPLOAD_BYTES_PER_DAEMON,
main.rs:57:     ScrollDirection, SharedImageBudget, SharedKittyUploadBudget, TerminalDamage, TerminalUpdate,
main.rs-58- };
main.rs-59- use tokio::{
main.rs-60-     fs,
main.rs-77- const IMAGE_CONTENT_IO_TIMEOUT: Duration = Duration::from_secs(5);
main.rs-78- const IMAGE_CONTENT_HEADER_BYTES: usize = 53;
main.rs-79- const IMAGE_MEMFD_HEADER_BYTES: usize = 45;
main.rs:80: const MAX_LIVE_SPLINTS: usize = 256;
main.rs-81- const TOPOLOGY_QUEUE: usize = 16;
main.rs-82- const OUTBOUND_QUEUE: usize = 32;
main.rs-83- const CONTROL_QUEUE: usize = 4;
main.rs-514- impl RuntimeRegistry {
main.rs-515-     fn insert(&mut self, runtime: LiveSplintRuntime) -> Result<(), LiveSplintRuntime> {
main.rs-516-         let id = runtime.handle().splint_id;
main.rs:517:         if self.entries.len() >= MAX_LIVE_SPLINTS || self.entries.contains_key(&id) {
main.rs-518-             return Err(runtime);
main.rs-519-         }
main.rs-520-         self.entries.insert(id, runtime);
main.rs-726-         revocations,
main.rs-727-         image_transfers: Mutex::new(TransferAdmission::default()),
main.rs-728-         image_transfer_expiry_changed: Notify::new(),
main.rs:729:         shared_image_budget: SharedImageBudget::new(MAX_IMAGE_BYTES_PER_DAEMON),
main.rs-730-         shared_kitty_upload_budget: SharedKittyUploadBudget::new(
main.rs-731-             DEFAULT_KITTY_UPLOAD_BYTES_PER_DAEMON,
main.rs-732-         ),
main.rs-996-     );
main.rs-997-     header[13..45].copy_from_slice(&metadata.digest);
main.rs-998-     header[45..49].copy_from_slice(
main.rs:999:         &u32::try_from(splinterm_protocol::MAX_IMAGE_CHUNK_BYTES)
main.rs-1000-             .expect("image chunk bound fits u32")
main.rs-1001-             .to_be_bytes(),
main.rs-1002-     );
main.rs-1001-             .to_be_bytes(),
main.rs-1002-     );
main.rs-1003-     header[49..53].copy_from_slice(
main.rs:1004:         &u32::try_from(splinterm_protocol::MAX_IMAGE_CHUNK_WINDOW)
main.rs-1005-             .expect("image chunk window fits u32")
main.rs-1006-             .to_be_bytes(),
main.rs-1007-     );
main.rs-1013-     while offset < pixels.len() {
main.rs-1014-         let window_end = offset
main.rs-1015-             .saturating_add(
main.rs:1016:                 splinterm_protocol::MAX_IMAGE_CHUNK_BYTES
main.rs-1017-                     * splinterm_protocol::MAX_IMAGE_CHUNK_WINDOW,
main.rs-1018-             )
main.rs-1019-             .min(pixels.len());
main.rs-1014-         let window_end = offset
main.rs-1015-             .saturating_add(
main.rs-1016-                 splinterm_protocol::MAX_IMAGE_CHUNK_BYTES
main.rs:1017:                     * splinterm_protocol::MAX_IMAGE_CHUNK_WINDOW,
main.rs-1018-             )
main.rs-1019-             .min(pixels.len());
main.rs-1020-         while offset < window_end {
main.rs-1019-             .min(pixels.len());
main.rs-1020-         while offset < window_end {
main.rs-1021-             let end = offset
main.rs:1022:                 .saturating_add(splinterm_protocol::MAX_IMAGE_CHUNK_BYTES)
main.rs-1023-                 .min(window_end);
main.rs-1024-             let mut chunk_header = [0_u8; 12];
main.rs-1025-             chunk_header[0..8].copy_from_slice(
main.rs-1373-                         subscription,
main.rs-1374-                     }) => {
main.rs-1375-                         if let Some(subscription) = subscription {
main.rs:1376:                             if subscriptions.len() >= MAX_SUBSCRIPTIONS {
main.rs-1377-                                 send_response(
main.rs-1378-                                     outbound,
main.rs-1379-                                     request_id,
main.rs-1493- struct SubscriptionAccess {
main.rs-1494-     grant_id: Option<u64>,
main.rs-1495-     maximum_returned_bytes: Option<usize>,
main.rs:1496:     scrollback_rows: usize,
main.rs-1497-     include_images: bool,
main.rs-1498-     history: HistoryState,
main.rs-1499-     visible_rows: Vec<splinterd::LiveRow>,
main.rs-1619-         splint_id,
main.rs-1620-         incarnation: handle.incarnation.value(),
main.rs-1621-         terminal_revision: snapshot.revision.value(),
main.rs:1622:         history_generation: snapshot.scrollback.history_generation,
main.rs-1623-     })
main.rs-1624- }
main.rs-1625- 
main.rs-1628-         matches!(
main.rs-1629-             scope,
main.rs-1630-             AccessScope::Observe
main.rs:1631:                 | AccessScope::Scrollback
main.rs-1632-                 | AccessScope::Input
main.rs-1633-                 | AccessScope::Resize
main.rs-1634-         )
main.rs-1822-     .env("SPLINTERM_WINDOW_ID", context.window.to_string())
main.rs-1823-     .env("SPLINTERM_SPLINT_ID", context.splint.to_string());
main.rs-1824-     let mut config = LiveSplintConfig::default();
main.rs:1825:     config.terminal.scrollback_lines = launch.scrollback_lines;
main.rs-1826-     config.terminal.shared_image_budget = Some(state.shared_image_budget.clone());
main.rs-1827-     config.terminal.shared_kitty_upload_budget = Some(state.shared_kitty_upload_budget.clone());
main.rs-1828-     config.incarnation_environment = Some(OsString::from("SPLINTERM_SPLINT_INCARNATION"));
main.rs-1843-     SplintLaunchMetadata {
main.rs-1844-         shell: launch.shell.clone(),
main.rs-1845-         login_shell: launch.login_shell,
main.rs:1846:         scrollback_lines: launch.scrollback_lines,
main.rs-1847-         ..SplintLaunchMetadata::default()
main.rs-1848-     }
main.rs-1849- }
main.rs-1854-         command: splint.command.clone(),
main.rs-1855-         shell: splint.launch.shell.clone(),
main.rs-1856-         login_shell: splint.launch.login_shell,
main.rs:1857:         scrollback_lines: splint.launch.scrollback_lines,
main.rs-1858-     }
main.rs-1859- }
main.rs-1860- 
main.rs-2093-     fn maximum_returned_bytes(&self) -> Option<usize> {
main.rs-2094-         self.policy_match
main.rs-2095-             .as_ref()
main.rs:2096:             .and_then(|matched| matched.max_returned_bytes)
main.rs-2097-     }
main.rs-2098- }
main.rs-2099- 
main.rs-2129-             | Request::RenameSplint { .. }
main.rs-2130-             | Request::Attach { .. }
main.rs-2131-             | Request::RequestImageContent { .. }
main.rs:2132:             | Request::StartScrollbackPage { .. }
main.rs-2133-             | Request::ScrollbackPage { .. }
main.rs-2134-             | Request::StartSearchScrollback { .. }
main.rs-2135-             | Request::SearchScrollback { .. }
main.rs-2130-             | Request::Attach { .. }
main.rs-2131-             | Request::RequestImageContent { .. }
main.rs-2132-             | Request::StartScrollbackPage { .. }
main.rs:2133:             | Request::ScrollbackPage { .. }
main.rs-2134-             | Request::StartSearchScrollback { .. }
main.rs-2135-             | Request::SearchScrollback { .. }
main.rs-2136-             | Request::AcquireControl { .. }
main.rs-2131-             | Request::RequestImageContent { .. }
main.rs-2132-             | Request::StartScrollbackPage { .. }
main.rs-2133-             | Request::ScrollbackPage { .. }
main.rs:2134:             | Request::StartSearchScrollback { .. }
main.rs-2135-             | Request::SearchScrollback { .. }
main.rs-2136-             | Request::AcquireControl { .. }
main.rs-2137-             | Request::SubscribeControl { .. }
main.rs-2132-             | Request::StartScrollbackPage { .. }
main.rs-2133-             | Request::ScrollbackPage { .. }
main.rs-2134-             | Request::StartSearchScrollback { .. }
main.rs:2135:             | Request::SearchScrollback { .. }
main.rs-2136-             | Request::AcquireControl { .. }
main.rs-2137-             | Request::SubscribeControl { .. }
main.rs-2138-             | Request::RequestControlTransfer { .. }
main.rs-2166-         request,
main.rs-2167-         Request::RequestAccess { .. }
main.rs-2168-             | Request::Attach { .. }
main.rs:2169:             | Request::StartScrollbackPage { .. }
main.rs-2170-             | Request::ScrollbackPage { .. }
main.rs-2171-             | Request::StartSearchScrollback { .. }
main.rs-2172-             | Request::SearchScrollback { .. }
main.rs-2167-         Request::RequestAccess { .. }
main.rs-2168-             | Request::Attach { .. }
main.rs-2169-             | Request::StartScrollbackPage { .. }
main.rs:2170:             | Request::ScrollbackPage { .. }
main.rs-2171-             | Request::StartSearchScrollback { .. }
main.rs-2172-             | Request::SearchScrollback { .. }
main.rs-2173-             | Request::AcquireControl { .. }
main.rs-2168-             | Request::Attach { .. }
main.rs-2169-             | Request::StartScrollbackPage { .. }
main.rs-2170-             | Request::ScrollbackPage { .. }
main.rs:2171:             | Request::StartSearchScrollback { .. }
main.rs-2172-             | Request::SearchScrollback { .. }
main.rs-2173-             | Request::AcquireControl { .. }
main.rs-2174-             | Request::Input { .. }
main.rs-2169-             | Request::StartScrollbackPage { .. }
main.rs-2170-             | Request::ScrollbackPage { .. }
main.rs-2171-             | Request::StartSearchScrollback { .. }
main.rs:2172:             | Request::SearchScrollback { .. }
main.rs-2173-             | Request::AcquireControl { .. }
main.rs-2174-             | Request::Input { .. }
main.rs-2175-             | Request::Resize { .. }
main.rs-2200-                     for scope in requested {
main.rs-2201-                         scopes.push(match scope {
main.rs-2202-                             AccessScope::Observe => Scope::TerminalVisibleRead,
main.rs:2203:                             AccessScope::Scrollback => Scope::ScrollbackRead,
main.rs-2204-                             AccessScope::Input => Scope::Input,
main.rs-2205-                             AccessScope::Resize => Scope::Resize,
main.rs-2206-                             AccessScope::Terminate => Scope::ProcessTerminate,
main.rs-2226-                         });
main.rs-2227-                     }
main.rs-2228-                 }
main.rs:2229:                 ConditionalRequirement::AttachScrollback => {
main.rs-2230-                     if matches!(request, Request::Attach { scrollback_rows, .. } if *scrollback_rows > 0)
main.rs-2231-                     {
main.rs-2232-                         scopes.push(Scope::ScrollbackRead);
main.rs-2227-                     }
main.rs-2228-                 }
main.rs-2229-                 ConditionalRequirement::AttachScrollback => {
main.rs:2230:                     if matches!(request, Request::Attach { scrollback_rows, .. } if *scrollback_rows > 0)
main.rs-2231-                     {
main.rs-2232-                         scopes.push(Scope::ScrollbackRead);
main.rs-2233-                     }
main.rs-2229-                 ConditionalRequirement::AttachScrollback => {
main.rs-2230-                     if matches!(request, Request::Attach { scrollback_rows, .. } if *scrollback_rows > 0)
main.rs-2231-                     {
main.rs:2232:                         scopes.push(Scope::ScrollbackRead);
main.rs-2233-                     }
main.rs-2234-                 }
main.rs-2235-                 ConditionalRequirement::LiveProcessTermination
main.rs-2253-     }
main.rs-2254-     match request {
main.rs-2255-         Request::Attach {
main.rs:2256:             scrollback_rows, ..
main.rs-2257-         } if *scrollback_rows > 0 => limits.returned_rows = Some(*scrollback_rows),
main.rs-2258-         Request::StartScrollbackPage { max_rows, .. }
main.rs-2259-         | Request::ScrollbackPage { max_rows, .. } => limits.returned_rows = Some(*max_rows),
main.rs-2254-     match request {
main.rs-2255-         Request::Attach {
main.rs-2256-             scrollback_rows, ..
main.rs:2257:         } if *scrollback_rows > 0 => limits.returned_rows = Some(*scrollback_rows),
main.rs-2258-         Request::StartScrollbackPage { max_rows, .. }
main.rs-2259-         | Request::ScrollbackPage { max_rows, .. } => limits.returned_rows = Some(*max_rows),
main.rs-2260-         Request::StartSearchScrollback { max_results, .. }
main.rs-2255-         Request::Attach {
main.rs-2256-             scrollback_rows, ..
main.rs-2257-         } if *scrollback_rows > 0 => limits.returned_rows = Some(*scrollback_rows),
main.rs:2258:         Request::StartScrollbackPage { max_rows, .. }
main.rs-2259-         | Request::ScrollbackPage { max_rows, .. } => limits.returned_rows = Some(*max_rows),
main.rs-2260-         Request::StartSearchScrollback { max_results, .. }
main.rs-2261-         | Request::SearchScrollback { max_results, .. } => {
main.rs-2256-             scrollback_rows, ..
main.rs-2257-         } if *scrollback_rows > 0 => limits.returned_rows = Some(*scrollback_rows),
main.rs-2258-         Request::StartScrollbackPage { max_rows, .. }
main.rs:2259:         | Request::ScrollbackPage { max_rows, .. } => limits.returned_rows = Some(*max_rows),
main.rs-2260-         Request::StartSearchScrollback { max_results, .. }
main.rs-2261-         | Request::SearchScrollback { max_results, .. } => {
main.rs-2262-             limits.results = Some(*max_results);
main.rs-2257-         } if *scrollback_rows > 0 => limits.returned_rows = Some(*scrollback_rows),
main.rs-2258-         Request::StartScrollbackPage { max_rows, .. }
main.rs-2259-         | Request::ScrollbackPage { max_rows, .. } => limits.returned_rows = Some(*max_rows),
main.rs:2260:         Request::StartSearchScrollback { max_results, .. }
main.rs-2261-         | Request::SearchScrollback { max_results, .. } => {
main.rs-2262-             limits.results = Some(*max_results);
main.rs-2263-             limits.deadline_ms =
main.rs-2258-         Request::StartScrollbackPage { max_rows, .. }
main.rs-2259-         | Request::ScrollbackPage { max_rows, .. } => limits.returned_rows = Some(*max_rows),
main.rs-2260-         Request::StartSearchScrollback { max_results, .. }
main.rs:2261:         | Request::SearchScrollback { max_results, .. } => {
main.rs-2262-             limits.results = Some(*max_results);
main.rs-2263-             limits.deadline_ms =
main.rs-2264-                 Some(u64::try_from(SEARCH_DEADLINE.as_millis()).unwrap_or(u64::MAX));
main.rs-2259-         | Request::ScrollbackPage { max_rows, .. } => limits.returned_rows = Some(*max_rows),
main.rs-2260-         Request::StartSearchScrollback { max_results, .. }
main.rs-2261-         | Request::SearchScrollback { max_results, .. } => {
main.rs:2262:             limits.results = Some(*max_results);
main.rs-2263-             limits.deadline_ms =
main.rs-2264-                 Some(u64::try_from(SEARCH_DEADLINE.as_millis()).unwrap_or(u64::MAX));
main.rs-2265-         }
main.rs-2263-             limits.deadline_ms =
main.rs-2264-                 Some(u64::try_from(SEARCH_DEADLINE.as_millis()).unwrap_or(u64::MAX));
main.rs-2265-         }
main.rs:2266:         Request::AuditInspect { max_records, .. } => limits.results = Some(*max_records),
main.rs-2267-         Request::CreateDojo { .. }
main.rs-2268-         | Request::CreateDojoAutomation { .. }
main.rs-2269-         | Request::SplitSplint { .. }
main.rs-2314-     })
main.rs-2315- }
main.rs-2316- 
main.rs:2317: async fn scrollback_response(
main.rs-2318-     state: &DaemonState,
main.rs-2319-     splint_id: SplintId,
main.rs-2320-     incarnation: u64,
main.rs-2318-     state: &DaemonState,
main.rs-2319-     splint_id: SplintId,
main.rs-2320-     incarnation: u64,
main.rs:2321:     page: LiveScrollbackPage,
main.rs-2322- ) -> Result<Response, ProtocolError> {
main.rs-2323-     let terminal_revision = page.terminal_revision.value();
main.rs-2324-     let history_generation = page.history_generation;
main.rs-2331-         page.title,
main.rs-2332-     )
main.rs-2333-     .await?;
main.rs:2334:     Ok(Response::ScrollbackPage {
main.rs-2335-         provenance,
main.rs-2336-         page: WireScrollbackPage {
main.rs-2337-             splint_id,
main.rs-2333-     .await?;
main.rs-2334-     Ok(Response::ScrollbackPage {
main.rs-2335-         provenance,
main.rs:2336:         page: WireScrollbackPage {
main.rs-2337-             splint_id,
main.rs-2338-             incarnation,
main.rs-2339-             terminal_revision,
main.rs-2346-     })
main.rs-2347- }
main.rs-2348- 
main.rs:2349: async fn scrollback_resync_response(
main.rs-2350-     state: &DaemonState,
main.rs-2351-     splint_id: SplintId,
main.rs-2352-     incarnation: u64,
main.rs-2350-     state: &DaemonState,
main.rs-2351-     splint_id: SplintId,
main.rs-2352-     incarnation: u64,
main.rs:2353:     page: LiveScrollbackPage,
main.rs-2354- ) -> Result<Response, ProtocolError> {
main.rs-2355-     let current_revision = page.terminal_revision.value();
main.rs-2356-     let history_generation = page.history_generation;
main.rs-2354- ) -> Result<Response, ProtocolError> {
main.rs-2355-     let current_revision = page.terminal_revision.value();
main.rs-2356-     let history_generation = page.history_generation;
main.rs:2357:     Ok(Response::ScrollbackResyncRequired {
main.rs-2358-         provenance: terminal_provenance(
main.rs-2359-             state,
main.rs-2360-             splint_id,
main.rs-2670-                     ..
main.rs-2671-                 },
main.rs-2672-         }
main.rs:2673:         | Request::ScrollbackPage {
main.rs-2674-             splint_id,
main.rs-2675-             incarnation,
main.rs-2676-             ..
main.rs-2675-             incarnation,
main.rs-2676-             ..
main.rs-2677-         }
main.rs:2678:         | Request::SearchScrollback {
main.rs-2679-             splint_id,
main.rs-2680-             incarnation,
main.rs-2681-             ..
main.rs-2713-             incarnation,
main.rs-2714-             ..
main.rs-2715-         }
main.rs:2716:         | Request::StartScrollbackPage {
main.rs-2717-             splint_id,
main.rs-2718-             incarnation,
main.rs-2719-             ..
main.rs-2718-             incarnation,
main.rs-2719-             ..
main.rs-2720-         }
main.rs:2721:         | Request::StartSearchScrollback {
main.rs-2722-             splint_id,
main.rs-2723-             incarnation,
main.rs-2724-             ..
main.rs-3074-             incarnation: None,
main.rs-3075-             ..
main.rs-3076-         }
main.rs:3077:         | Request::StartScrollbackPage {
main.rs-3078-             splint_id,
main.rs-3079-             incarnation: None,
main.rs-3080-             ..
main.rs-3079-             incarnation: None,
main.rs-3080-             ..
main.rs-3081-         }
main.rs:3082:         | Request::StartSearchScrollback {
main.rs-3083-             splint_id,
main.rs-3084-             incarnation: None,
main.rs-3085-             ..
main.rs-3109-     };
main.rs-3110-     match &mut request {
main.rs-3111-         Request::Attach { incarnation, .. }
main.rs:3112:         | Request::StartScrollbackPage { incarnation, .. }
main.rs-3113-         | Request::StartSearchScrollback { incarnation, .. } => {
main.rs-3114-             *incarnation = Some(current);
main.rs-3115-         }
main.rs-3110-     match &mut request {
main.rs-3111-         Request::Attach { incarnation, .. }
main.rs-3112-         | Request::StartScrollbackPage { incarnation, .. }
main.rs:3113:         | Request::StartSearchScrollback { incarnation, .. } => {
main.rs-3114-             *incarnation = Some(current);
main.rs-3115-         }
main.rs-3116-         _ => unreachable!("only current terminal requests are bound"),
main.rs-3297-         command: launch.argv,
main.rs-3298-         shell: None,
main.rs-3299-         login_shell: false,
main.rs:3300:         scrollback_lines: splinterm_terminal::TerminalConfig::default().scrollback_lines,
main.rs-3301-     };
main.rs-3302-     launch.validate()?;
main.rs-3303-     Ok(launch)
main.rs-3541-         } => {
main.rs-3542-             let _ = current_handle(state, splint_id, incarnation).await?;
main.rs-3543-             let canonical: std::collections::BTreeSet<_> = scopes.into_iter().collect();
main.rs:3544:             if canonical.is_empty() || canonical.len() > splinterm_protocol::MAX_ACCESS_SCOPES {
main.rs-3545-                 return Err(invalid("access scopes are empty or exceed limits"));
main.rs-3546-             }
main.rs-3547-             let scopes: Vec<_> = canonical.into_iter().collect();
main.rs-3806-                     return Err(not_found());
main.rs-3807-                 }
main.rs-3808-             }
main.rs:3809:             if state.runtimes.lock().await.entries.len() >= MAX_LIVE_SPLINTS {
main.rs-3810-                 return Err(ProtocolError::new(
main.rs-3811-                     ErrorCode::ResourceLimit,
main.rs-3812-                     "live Splint registry is full",
main.rs-4208-         Request::Attach {
main.rs-4209-             splint_id,
main.rs-4210-             incarnation,
main.rs:4211:             scrollback_rows,
main.rs-4212-         } => {
main.rs-4213-             let incarnation = incarnation.ok_or_else(internal)?;
main.rs-4214-             let include_images =
main.rs-4213-             let incarnation = incarnation.ok_or_else(internal)?;
main.rs-4214-             let include_images =
main.rs-4215-                 include_image_metadata(trusted_ui_client, peer.is_matching_splinterm());
main.rs:4216:             let required = if scrollback_rows == 0 {
main.rs-4217-                 vec![AccessScope::Observe]
main.rs-4218-             } else {
main.rs-4219-                 vec![AccessScope::Observe, AccessScope::Scrollback]
main.rs-4216-             let required = if scrollback_rows == 0 {
main.rs-4217-                 vec![AccessScope::Observe]
main.rs-4218-             } else {
main.rs:4219:                 vec![AccessScope::Observe, AccessScope::Scrollback]
main.rs-4220-             };
main.rs-4221-             let grant_id = if authorization.policy_authorized() {
main.rs-4222-                 None
main.rs-4232-                 .await?
main.rs-4233-             };
main.rs-4234-             let handle = current_handle(state, splint_id, incarnation).await?;
main.rs:4235:             let scrollback_rows = scrollback_rows.min(MAX_SNAPSHOT_SCROLLBACK_ROWS);
main.rs-4236-             let (snapshot, subscription) = handle
main.rs-4237-                 .attach_compact_with_scrollback(scrollback_rows)
main.rs-4238-                 .await
main.rs-4234-             let handle = current_handle(state, splint_id, incarnation).await?;
main.rs-4235-             let scrollback_rows = scrollback_rows.min(MAX_SNAPSHOT_SCROLLBACK_ROWS);
main.rs-4236-             let (snapshot, subscription) = handle
main.rs:4237:                 .attach_compact_with_scrollback(scrollback_rows)
main.rs-4238-                 .await
main.rs-4239-                 .map_err(|_| internal())?;
main.rs-4240-             let id = NEXT_SUBSCRIPTION.fetch_add(1, Ordering::Relaxed);
main.rs-4245-                 splint_id,
main.rs-4246-                 incarnation,
main.rs-4247-                 snapshot.revision.value(),
main.rs:4248:                 snapshot.scrollback.history_generation,
main.rs-4249-                 snapshot.title.clone(),
main.rs-4250-             )
main.rs-4251-             .await?;
main.rs-4262-                     access: SubscriptionAccess {
main.rs-4263-                         grant_id,
main.rs-4264-                         maximum_returned_bytes: authorization.maximum_returned_bytes(),
main.rs:4265:                         scrollback_rows,
main.rs-4266-                         include_images,
main.rs-4267-                         history,
main.rs-4268-                         visible_rows,
main.rs-4270-                 }),
main.rs-4271-             });
main.rs-4272-         }
main.rs:4273:         Request::StartScrollbackPage {
main.rs-4274-             splint_id,
main.rs-4275-             incarnation,
main.rs-4276-             max_rows,
main.rs-4273-         Request::StartScrollbackPage {
main.rs-4274-             splint_id,
main.rs-4275-             incarnation,
main.rs:4276:             max_rows,
main.rs-4277-         } => {
main.rs-4278-             let incarnation = incarnation.ok_or_else(internal)?;
main.rs-4279-             if max_rows == 0 || max_rows > MAX_SCROLLBACK_PAGE_ROWS {
main.rs-4276-             max_rows,
main.rs-4277-         } => {
main.rs-4278-             let incarnation = incarnation.ok_or_else(internal)?;
main.rs:4279:             if max_rows == 0 || max_rows > MAX_SCROLLBACK_PAGE_ROWS {
main.rs-4280-                 return Err(ProtocolError::new(
main.rs-4281-                     ErrorCode::InvalidArgument,
main.rs-4282-                     "scrollback page request exceeds protocol bounds",
main.rs-4279-             if max_rows == 0 || max_rows > MAX_SCROLLBACK_PAGE_ROWS {
main.rs-4280-                 return Err(ProtocolError::new(
main.rs-4281-                     ErrorCode::InvalidArgument,
main.rs:4282:                     "scrollback page request exceeds protocol bounds",
main.rs-4283-                 ));
main.rs-4284-             }
main.rs-4285-             if !authorization.policy_authorized() {
main.rs-4289-                     trusted_ui_client,
main.rs-4290-                     splint_id,
main.rs-4291-                     incarnation,
main.rs:4292:                     &[AccessScope::Observe, AccessScope::Scrollback],
main.rs-4293-                 )
main.rs-4294-                 .await?;
main.rs-4295-             }
main.rs-4295-             }
main.rs-4296-             let page = current_handle(state, splint_id, incarnation)
main.rs-4297-                 .await?
main.rs:4298:                 .start_scrollback_page(max_rows)
main.rs-4299-                 .await
main.rs-4300-                 .map_err(|_| internal())?;
main.rs-4301-             scrollback_response(state, splint_id, incarnation, page).await?
main.rs-4298-                 .start_scrollback_page(max_rows)
main.rs-4299-                 .await
main.rs-4300-                 .map_err(|_| internal())?;
main.rs:4301:             scrollback_response(state, splint_id, incarnation, page).await?
main.rs-4302-         }
main.rs-4303-         Request::ScrollbackPage {
main.rs-4304-             splint_id,
main.rs-4300-                 .map_err(|_| internal())?;
main.rs-4301-             scrollback_response(state, splint_id, incarnation, page).await?
main.rs-4302-         }
main.rs:4303:         Request::ScrollbackPage {
main.rs-4304-             splint_id,
main.rs-4305-             incarnation,
main.rs-4306-             terminal_revision,
main.rs-4306-             terminal_revision,
main.rs-4307-             history_generation,
main.rs-4308-             before_row_id,
main.rs:4309:             max_rows,
main.rs-4310-         } => {
main.rs-4311-             if before_row_id == 0 || max_rows == 0 || max_rows > MAX_SCROLLBACK_PAGE_ROWS {
main.rs-4312-                 return Err(ProtocolError::new(
main.rs-4308-             before_row_id,
main.rs-4309-             max_rows,
main.rs-4310-         } => {
main.rs:4311:             if before_row_id == 0 || max_rows == 0 || max_rows > MAX_SCROLLBACK_PAGE_ROWS {
main.rs-4312-                 return Err(ProtocolError::new(
main.rs-4313-                     ErrorCode::InvalidArgument,
main.rs-4314-                     "scrollback page request exceeds protocol bounds",
main.rs-4311-             if before_row_id == 0 || max_rows == 0 || max_rows > MAX_SCROLLBACK_PAGE_ROWS {
main.rs-4312-                 return Err(ProtocolError::new(
main.rs-4313-                     ErrorCode::InvalidArgument,
main.rs:4314:                     "scrollback page request exceeds protocol bounds",
main.rs-4315-                 ));
main.rs-4316-             }
main.rs-4317-             if !authorization.policy_authorized() {
main.rs-4321-                     trusted_ui_client,
main.rs-4322-                     splint_id,
main.rs-4323-                     incarnation,
main.rs:4324:                     &[AccessScope::Observe, AccessScope::Scrollback],
main.rs-4325-                 )
main.rs-4326-                 .await?;
main.rs-4327-             }
main.rs-4327-             }
main.rs-4328-             let page = current_handle(state, splint_id, incarnation)
main.rs-4329-                 .await?
main.rs:4330:                 .scrollback_page(before_row_id, max_rows)
main.rs-4331-                 .await
main.rs-4332-                 .map_err(|_| internal())?;
main.rs-4333-             if page.terminal_revision.value() != terminal_revision
main.rs-4334-                 || page.history_generation != history_generation
main.rs-4335-             {
main.rs-4336-                 return Ok(Handled {
main.rs:4337:                     response: scrollback_resync_response(state, splint_id, incarnation, page)
main.rs-4338-                         .await?,
main.rs-4339-                     subscription: None,
main.rs-4340-                 });
main.rs-4339-                     subscription: None,
main.rs-4340-                 });
main.rs-4341-             }
main.rs:4342:             scrollback_response(state, splint_id, incarnation, page).await?
main.rs-4343-         }
main.rs-4344-         Request::StartSearchScrollback {
main.rs-4345-             splint_id,
main.rs-4341-             }
main.rs-4342-             scrollback_response(state, splint_id, incarnation, page).await?
main.rs-4343-         }
main.rs:4344:         Request::StartSearchScrollback {
main.rs-4345-             splint_id,
main.rs-4346-             incarnation,
main.rs-4347-             query,
main.rs-4346-             incarnation,
main.rs-4347-             query,
main.rs-4348-             case_sensitive,
main.rs:4349:             max_results,
main.rs-4350-         } => {
main.rs-4351-             let incarnation = incarnation.ok_or_else(internal)?;
main.rs-4352-             if query.is_empty()
main.rs-4350-         } => {
main.rs-4351-             let incarnation = incarnation.ok_or_else(internal)?;
main.rs-4352-             if query.is_empty()
main.rs:4353:                 || query.len() > MAX_SEARCH_QUERY_BYTES
main.rs-4354-                 || max_results == 0
main.rs-4355-                 || max_results > MAX_SEARCH_RESULTS
main.rs-4356-             {
main.rs-4351-             let incarnation = incarnation.ok_or_else(internal)?;
main.rs-4352-             if query.is_empty()
main.rs-4353-                 || query.len() > MAX_SEARCH_QUERY_BYTES
main.rs:4354:                 || max_results == 0
main.rs-4355-                 || max_results > MAX_SEARCH_RESULTS
main.rs-4356-             {
main.rs-4357-                 return Err(invalid("search request exceeds protocol bounds"));
main.rs-4352-             if query.is_empty()
main.rs-4353-                 || query.len() > MAX_SEARCH_QUERY_BYTES
main.rs-4354-                 || max_results == 0
main.rs:4355:                 || max_results > MAX_SEARCH_RESULTS
main.rs-4356-             {
main.rs-4357-                 return Err(invalid("search request exceeds protocol bounds"));
main.rs-4358-             }
main.rs-4363-                     trusted_ui_client,
main.rs-4364-                     splint_id,
main.rs-4365-                     incarnation,
main.rs:4366:                     &[AccessScope::Observe, AccessScope::Scrollback],
main.rs-4367-                 )
main.rs-4368-                 .await?;
main.rs-4369-             }
main.rs-4369-             }
main.rs-4370-             let search = current_handle(state, splint_id, incarnation)
main.rs-4371-                 .await?
main.rs:4372:                 .search(query, case_sensitive, 0, max_results, SEARCH_DEADLINE)
main.rs-4373-                 .await
main.rs-4374-                 .map_err(|_| internal())?;
main.rs-4375-             search_response(state, splint_id, incarnation, search).await?
main.rs-4374-                 .map_err(|_| internal())?;
main.rs-4375-             search_response(state, splint_id, incarnation, search).await?
main.rs-4376-         }
main.rs:4377:         Request::SearchScrollback {
main.rs-4378-             splint_id,
main.rs-4379-             incarnation,
main.rs-4380-             terminal_revision,
main.rs-4382-             query,
main.rs-4383-             case_sensitive,
main.rs-4384-             cursor,
main.rs:4385:             max_results,
main.rs-4386-         } => {
main.rs-4387-             if query.is_empty()
main.rs-4388-                 || query.len() > MAX_SEARCH_QUERY_BYTES
main.rs-4385-             max_results,
main.rs-4386-         } => {
main.rs-4387-             if query.is_empty()
main.rs:4388:                 || query.len() > MAX_SEARCH_QUERY_BYTES
main.rs-4389-                 || max_results == 0
main.rs-4390-                 || max_results > MAX_SEARCH_RESULTS
main.rs-4391-             {
main.rs-4386-         } => {
main.rs-4387-             if query.is_empty()
main.rs-4388-                 || query.len() > MAX_SEARCH_QUERY_BYTES
main.rs:4389:                 || max_results == 0
main.rs-4390-                 || max_results > MAX_SEARCH_RESULTS
main.rs-4391-             {
main.rs-4392-                 return Err(invalid("search request exceeds protocol bounds"));
main.rs-4387-             if query.is_empty()
main.rs-4388-                 || query.len() > MAX_SEARCH_QUERY_BYTES
main.rs-4389-                 || max_results == 0
main.rs:4390:                 || max_results > MAX_SEARCH_RESULTS
main.rs-4391-             {
main.rs-4392-                 return Err(invalid("search request exceeds protocol bounds"));
main.rs-4393-             }
main.rs-4399-                     trusted_ui_client,
main.rs-4400-                     splint_id,
main.rs-4401-                     incarnation,
main.rs:4402:                     &[AccessScope::Observe, AccessScope::Scrollback],
main.rs-4403-                 )
main.rs-4404-                 .await?;
main.rs-4405-             }
main.rs-4409-                     query,
main.rs-4410-                     case_sensitive,
main.rs-4411-                     skip_rows,
main.rs:4412:                     max_results,
main.rs-4413-                     SEARCH_DEADLINE,
main.rs-4414-                 )
main.rs-4415-                 .await
main.rs-4652-                 )
main.rs-4653-                 .await?;
main.rs-4654-             }
main.rs:4655:             if bytes.len() > MAX_INPUT_BYTES {
main.rs-4656-                 return Err(invalid("input exceeds limit"));
main.rs-4657-             }
main.rs-4658-             let handle =
[100 matches limit reached. Use limit=200 for more, or refine pattern]
wayland.rs-112- use crate::renderer::{
wayland.rs-113-     ChromeText, CursorPresentation, HistoryOverlayStatus, SnapshotFrame, SnapshotOverlays, TextRow,
wayland.rs-114-     configured_background_bgra, history_overlay_layout, paint, paint_box_drawing_cell,
wayland.rs-115-     paint_history_overlay, paint_snapshot_overlays, paint_snapshot_presented,
wayland.rs:116:     paint_snapshot_region_presented, paint_snapshot_rows_presented, scroll_snapshot_pixels,
wayland.rs-117-     set_background_alpha, set_font_zoom_steps, snapshot_row_rect, update_output_dpi, write_ppm,
wayland.rs-118- };
wayland.rs-119- use crate::viewport::ScrollbackViewport;
wayland.rs-120- 
wayland.rs-115-     paint_history_overlay, paint_snapshot_overlays, paint_snapshot_presented,
wayland.rs-116-     paint_snapshot_region_presented, paint_snapshot_rows_presented, scroll_snapshot_pixels,
wayland.rs-117-     set_background_alpha, set_font_zoom_steps, snapshot_row_rect, update_output_dpi, write_ppm,
wayland.rs-118- };
wayland.rs:119: use crate::viewport::ScrollbackViewport;
wayland.rs-120- 
wayland.rs-121- const INITIAL_WIDTH: u32 = 960;
wayland.rs-122- const INITIAL_HEIGHT: u32 = 600;
wayland.rs-123- const TEXT_MIMES: [&str; 3] = ["text/plain;charset=utf-8", "text/plain", "UTF8_STRING"];
wayland.rs-128- const CLIPBOARD_IO_TIMEOUT: Duration = Duration::from_secs(2);
wayland.rs-129- // Keep application mouse reports at one report per wheel step. Local history
wayland.rs-130- // follows Foot's default three-lines-per-step semantic distance; visual motion
wayland.rs-131- // must be smoothed in pixels rather than by increasing this row multiplier.
wayland.rs:132: const SCROLLBACK_WHEEL_MULTIPLIER: f64 = 3.0;
wayland.rs-133- const WHEEL_VALUE120_STEP: f64 = 120.0;
wayland.rs-134- const SCALE_DENOMINATOR: u32 = 120;
wayland.rs-135- const MIN_SCALE_120: u32 = 120;
wayland.rs-136- const MAX_SCALE_120: u32 = 960;
wayland.rs-445-     Update {
wayland.rs-446-         update: TerminalUpdate,
wayland.rs-447-         image_sources: Option<ImageContentLeaseSet>,
wayland.rs-448-     },
wayland.rs:449:     ScrollbackPages(Vec<splinterm_protocol::ScrollbackPage>),
wayland.rs-450-     ScrollbackResyncRequired,
wayland.rs-451-     Authority(AuthorityStatus),
wayland.rs-452-     Control(bool),
wayland.rs-453-     ControlTransferRequested(u64),
wayland.rs-446-         update: TerminalUpdate,
wayland.rs-447-         image_sources: Option<ImageContentLeaseSet>,
wayland.rs-448-     },
wayland.rs-449-     ScrollbackPages(Vec<splinterm_protocol::ScrollbackPage>),
wayland.rs:450:     ScrollbackResyncRequired,
wayland.rs-451-     Authority(AuthorityStatus),
wayland.rs-452-     Control(bool),
wayland.rs-453-     ControlTransferRequested(u64),
wayland.rs-454-     ControlTransferResolved(ControlTransferOutcome),
wayland.rs-473-         rows: u16,
wayland.rs-474-         pixel_width: u16,
wayland.rs-475-         pixel_height: u16,
wayland.rs-476-     },
wayland.rs:477:     FetchScrollback {
wayland.rs-478-         splint_id: SplintId,
wayland.rs-479-         incarnation: u64,
wayland.rs-480-         terminal_revision: u64,
wayland.rs-481-         history_generation: u64,
wayland.rs-805-         pane: PaneView {
wayland.rs-806-             snapshot: options.snapshot,
wayland.rs-807-             snapshot_frame,
wayland.rs-808-             image_sources: options.image_sources,
wayland.rs:809:             scrollback_viewport: ScrollbackViewport::default(),
wayland.rs-810-             painted_history_status: None,
wayland.rs-811-             history_page_pending: false,
wayland.rs-812-             history_selection_pin_blocked: false,
wayland.rs-813-             scroll_started_at: None,
wayland.rs-809-             scrollback_viewport: ScrollbackViewport::default(),
wayland.rs-810-             painted_history_status: None,
wayland.rs-811-             history_page_pending: false,
wayland.rs-812-             history_selection_pin_blocked: false,
wayland.rs:813:             scroll_started_at: None,
wayland.rs-814-             rendered_viewport_offset: 0,
wayland.rs-815-             viewport_dirty: false,
wayland.rs-816-             updates: options.updates,
wayland.rs-817-             commands: options.commands,
wayland.rs-822-             last_resize: None,
wayland.rs-823-             prepare_dirty_rows: Vec::new(),
wayland.rs-824-             raster_dirty_rows: Vec::new(),
wayland.rs-825-             surface_dirty_rows: Vec::new(),
wayland.rs:826:             pending_scrolls: Vec::new(),
wayland.rs-827-             selected_text: None,
wayland.rs-828-             selection: None,
wayland.rs-829-             selecting: false,
wayland.rs-830-             pointer_cell: None,
wayland.rs-838-         topology_updates: options.topology_updates,
wayland.rs-839-         topology_commands: options.topology_commands,
wayland.rs-840-         signoff,
wayland.rs-841-         graphical_input_probe,
wayland.rs:842:         scroll_trace: std::env::var_os("SPLINTERM_SCROLL_TRACE").is_some(),
wayland.rs-843-         trusted_consent,
wayland.rs-844-         cursor_style: options.cursor_style,
wayland.rs-845-         cursor_blink: options.cursor_blink,
wayland.rs-846-         title_override: options.title,
wayland.rs-870-         clipboard_rx,
wayland.rs-871-         last_pointer_serial: None,
wayland.rs-872-         pressed_buttons: HashMap::new(),
wayland.rs-873-         vertical_wheel: WheelAccumulator::default(),
wayland.rs:874:         scrollback_wheel: WheelAccumulator::default(),
wayland.rs-875-         loop_handle: event_loop.handle(),
wayland.rs-876-         update_waker,
wayland.rs-877-         logical_width: initial_width,
wayland.rs-878-         logical_height: initial_height,
wayland.rs-1437- struct PaneView {
wayland.rs-1438-     snapshot: Option<TerminalSnapshot>,
wayland.rs-1439-     snapshot_frame: Option<SnapshotFrame>,
wayland.rs-1440-     image_sources: ImageContentLeaseSet,
wayland.rs:1441:     scrollback_viewport: ScrollbackViewport,
wayland.rs-1442-     painted_history_status: Option<HistoryOverlayStatus>,
wayland.rs-1443-     history_page_pending: bool,
wayland.rs-1444-     history_selection_pin_blocked: bool,
wayland.rs-1445-     scroll_started_at: Option<Instant>,
wayland.rs-1441-     scrollback_viewport: ScrollbackViewport,
wayland.rs-1442-     painted_history_status: Option<HistoryOverlayStatus>,
wayland.rs-1443-     history_page_pending: bool,
wayland.rs-1444-     history_selection_pin_blocked: bool,
wayland.rs:1445:     scroll_started_at: Option<Instant>,
wayland.rs-1446-     rendered_viewport_offset: usize,
wayland.rs-1447-     viewport_dirty: bool,
wayland.rs-1448-     updates: Option<Receiver<WindowUpdate>>,
wayland.rs-1449-     commands: Option<Sender<WindowCommand>>,
wayland.rs-1454-     last_resize: Option<(u16, u16, u16, u16)>,
wayland.rs-1455-     prepare_dirty_rows: Vec<bool>,
wayland.rs-1456-     raster_dirty_rows: Vec<bool>,
wayland.rs-1457-     surface_dirty_rows: Vec<bool>,
wayland.rs:1458:     pending_scrolls: Vec<splinterm_protocol::TerminalScroll>,
wayland.rs-1459-     selected_text: Option<Vec<u8>>,
wayland.rs-1460-     selection: Option<Selection>,
wayland.rs-1461-     selecting: bool,
wayland.rs-1462-     pointer_cell: Option<CellPosition>,
wayland.rs-1471-         &display,
wayland.rs-1472-         scale_120,
wayland.rs-1473-         Some(&pane.image_sources),
wayland.rs-1474-     )?);
wayland.rs:1475:     pane.rendered_viewport_offset = pane.scrollback_viewport.offset_from_bottom();
wayland.rs-1476-     pane.viewport_dirty = false;
wayland.rs-1477-     Ok(true)
wayland.rs-1478- }
wayland.rs-1479- 
wayland.rs-1487-         Ok(Self {
wayland.rs-1488-             snapshot: Some(options.snapshot),
wayland.rs-1489-             snapshot_frame,
wayland.rs-1490-             image_sources: options.image_sources,
wayland.rs:1491:             scrollback_viewport: ScrollbackViewport::default(),
wayland.rs-1492-             painted_history_status: None,
wayland.rs-1493-             history_page_pending: false,
wayland.rs-1494-             history_selection_pin_blocked: false,
wayland.rs-1495-             scroll_started_at: None,
wayland.rs-1491-             scrollback_viewport: ScrollbackViewport::default(),
wayland.rs-1492-             painted_history_status: None,
wayland.rs-1493-             history_page_pending: false,
wayland.rs-1494-             history_selection_pin_blocked: false,
wayland.rs:1495:             scroll_started_at: None,
wayland.rs-1496-             rendered_viewport_offset: 0,
wayland.rs-1497-             viewport_dirty: false,
wayland.rs-1498-             updates: Some(options.updates),
wayland.rs-1499-             commands: Some(options.commands),
wayland.rs-1504-             last_resize: None,
wayland.rs-1505-             prepare_dirty_rows: Vec::new(),
wayland.rs-1506-             raster_dirty_rows: Vec::new(),
wayland.rs-1507-             surface_dirty_rows: Vec::new(),
wayland.rs:1508:             pending_scrolls: Vec::new(),
wayland.rs-1509-             selected_text: None,
wayland.rs-1510-             selection: None,
wayland.rs-1511-             selecting: false,
wayland.rs-1512-             pointer_cell: None,
wayland.rs-1523-     }
wayland.rs-1524- 
wayland.rs-1525-     fn display_snapshot(&self) -> Option<TerminalSnapshot> {
wayland.rs-1526-         let snapshot = self.snapshot.as_ref()?;
wayland.rs:1527:         if self.scrollback_viewport.is_live() {
wayland.rs-1528-             return Some(snapshot.clone());
wayland.rs-1529-         }
wayland.rs-1530-         let mut display = snapshot.clone();
wayland.rs-1531-         let cursor_row = viewport_cursor_row(
wayland.rs-1529-         }
wayland.rs-1530-         let mut display = snapshot.clone();
wayland.rs-1531-         let cursor_row = viewport_cursor_row(
wayland.rs-1532-             snapshot.cursor_row,
wayland.rs:1533:             self.scrollback_viewport.offset_from_bottom(),
wayland.rs-1534-             snapshot.rows,
wayland.rs-1535-         );
wayland.rs-1536-         if cursor_row.is_none() {
wayland.rs-1537-             display.input_modes.cursor_visible = false;
wayland.rs-1539-         display.cursor_column = cursor_row.map_or(-1, |_| snapshot.cursor_column);
wayland.rs-1540-         display.cursor_row = cursor_row.unwrap_or(-1);
wayland.rs-1541-         display.cursor_deferred_wrap = false;
wayland.rs-1542-         display.visible_rows = self
wayland.rs:1543:             .scrollback_viewport
wayland.rs-1544-             .visible_rows(snapshot)
wayland.rs-1545-             .into_iter()
wayland.rs-1546-             .cloned()
wayland.rs-1547-             .collect();
wayland.rs-1544-             .visible_rows(snapshot)
wayland.rs-1545-             .into_iter()
wayland.rs-1546-             .cloned()
wayland.rs-1547-             .collect();
wayland.rs:1548:         display.oldest_available_scrollback_row_id = None;
wayland.rs-1549-         display.newest_available_scrollback_row_id = None;
wayland.rs-1550-         display.scrollback_rows.clear();
wayland.rs-1551-         display.omitted_oldest_scrollback_rows = display.available_scrollback_rows;
wayland.rs-1552-         Some(display)
wayland.rs-1545-             .into_iter()
wayland.rs-1546-             .cloned()
wayland.rs-1547-             .collect();
wayland.rs-1548-         display.oldest_available_scrollback_row_id = None;
wayland.rs:1549:         display.newest_available_scrollback_row_id = None;
wayland.rs-1550-         display.scrollback_rows.clear();
wayland.rs-1551-         display.omitted_oldest_scrollback_rows = display.available_scrollback_rows;
wayland.rs-1552-         Some(display)
wayland.rs-1553-     }
wayland.rs-1546-             .cloned()
wayland.rs-1547-             .collect();
wayland.rs-1548-         display.oldest_available_scrollback_row_id = None;
wayland.rs-1549-         display.newest_available_scrollback_row_id = None;
wayland.rs:1550:         display.scrollback_rows.clear();
wayland.rs-1551-         display.omitted_oldest_scrollback_rows = display.available_scrollback_rows;
wayland.rs-1552-         Some(display)
wayland.rs-1553-     }
wayland.rs-1554- 
wayland.rs-1547-             .collect();
wayland.rs-1548-         display.oldest_available_scrollback_row_id = None;
wayland.rs-1549-         display.newest_available_scrollback_row_id = None;
wayland.rs-1550-         display.scrollback_rows.clear();
wayland.rs:1551:         display.omitted_oldest_scrollback_rows = display.available_scrollback_rows;
wayland.rs-1552-         Some(display)
wayland.rs-1553-     }
wayland.rs-1554- 
wayland.rs-1555-     fn apply_background_pages(
wayland.rs-1553-     }
wayland.rs-1554- 
wayland.rs-1555-     fn apply_background_pages(
wayland.rs-1556-         &mut self,
wayland.rs:1557:         pages: Vec<splinterm_protocol::ScrollbackPage>,
wayland.rs-1558-     ) -> Result<bool> {
wayland.rs-1559-         self.history_page_pending = false;
wayland.rs-1560-         let pinned = self
wayland.rs-1561-             .selection
wayland.rs-1562-             .map(|selection| [selection.anchor.row_id, selection.end.row_id]);
wayland.rs-1563-         let snapshot = self
wayland.rs-1564-             .snapshot
wayland.rs-1565-             .as_mut()
wayland.rs:1566:             .context("scrollback pages arrived before initial pane snapshot")?;
wayland.rs-1567-         if pages.iter().any(|page| {
wayland.rs-1568-             page.splint_id != snapshot.splint_id
wayland.rs-1569-                 || page.incarnation != snapshot.incarnation
wayland.rs-1570-                 || page.terminal_revision != snapshot.revision
wayland.rs-1572-         }) {
wayland.rs-1573-             return Ok(false);
wayland.rs-1574-         }
wayland.rs-1575-         let first_loaded = snapshot
wayland.rs:1576:             .scrollback_rows
wayland.rs-1577-             .first()
wayland.rs-1578-             .and_then(|row| row.row_id)
wayland.rs-1579-             .unwrap_or(u64::MAX);
wayland.rs-1580-         let existing = snapshot
wayland.rs-1577-             .first()
wayland.rs-1578-             .and_then(|row| row.row_id)
wayland.rs-1579-             .unwrap_or(u64::MAX);
wayland.rs-1580-         let existing = snapshot
wayland.rs:1581:             .scrollback_rows
wayland.rs-1582-             .iter()
wayland.rs-1583-             .filter_map(|row| row.row_id)
wayland.rs-1584-             .collect::<std::collections::BTreeSet<_>>();
wayland.rs-1585-         let metadata = pages
wayland.rs-1596-             .collect::<Vec<_>>();
wayland.rs-1597-         if older.is_empty() {
wayland.rs-1598-             return Ok(false);
wayland.rs-1599-         }
wayland.rs:1600:         older.extend(snapshot.scrollback_rows.iter().cloned());
wayland.rs-1601-         let Some(older) = bound_history_page_with_pins(older, pinned, &snapshot.visible_rows)
wayland.rs-1602-         else {
wayland.rs-1603-             self.history_selection_pin_blocked = true;
wayland.rs-1604-             return Ok(false);
wayland.rs-1602-         else {
wayland.rs-1603-             self.history_selection_pin_blocked = true;
wayland.rs-1604-             return Ok(false);
wayland.rs-1605-         };
wayland.rs:1606:         snapshot.scrollback_rows = older;
wayland.rs-1607-         snapshot.omitted_oldest_scrollback_rows = omitted_rows_before_cache(
wayland.rs-1608-             snapshot.oldest_available_scrollback_row_id,
wayland.rs-1609-             &snapshot.scrollback_rows,
wayland.rs-1610-             snapshot.available_scrollback_rows,
wayland.rs-1603-             self.history_selection_pin_blocked = true;
wayland.rs-1604-             return Ok(false);
wayland.rs-1605-         };
wayland.rs-1606-         snapshot.scrollback_rows = older;
wayland.rs:1607:         snapshot.omitted_oldest_scrollback_rows = omitted_rows_before_cache(
wayland.rs-1608-             snapshot.oldest_available_scrollback_row_id,
wayland.rs-1609-             &snapshot.scrollback_rows,
wayland.rs-1610-             snapshot.available_scrollback_rows,
wayland.rs-1611-         );
wayland.rs-1604-             return Ok(false);
wayland.rs-1605-         };
wayland.rs-1606-         snapshot.scrollback_rows = older;
wayland.rs-1607-         snapshot.omitted_oldest_scrollback_rows = omitted_rows_before_cache(
wayland.rs:1608:             snapshot.oldest_available_scrollback_row_id,
wayland.rs-1609-             &snapshot.scrollback_rows,
wayland.rs-1610-             snapshot.available_scrollback_rows,
wayland.rs-1611-         );
wayland.rs-1612-         if let Some((oldest, newest)) = metadata {
wayland.rs-1605-         };
wayland.rs-1606-         snapshot.scrollback_rows = older;
wayland.rs-1607-         snapshot.omitted_oldest_scrollback_rows = omitted_rows_before_cache(
wayland.rs-1608-             snapshot.oldest_available_scrollback_row_id,
wayland.rs:1609:             &snapshot.scrollback_rows,
wayland.rs-1610-             snapshot.available_scrollback_rows,
wayland.rs-1611-         );
wayland.rs-1612-         if let Some((oldest, newest)) = metadata {
wayland.rs-1613-             snapshot.oldest_available_scrollback_row_id = oldest;
wayland.rs-1606-         snapshot.scrollback_rows = older;
wayland.rs-1607-         snapshot.omitted_oldest_scrollback_rows = omitted_rows_before_cache(
wayland.rs-1608-             snapshot.oldest_available_scrollback_row_id,
wayland.rs-1609-             &snapshot.scrollback_rows,
wayland.rs:1610:             snapshot.available_scrollback_rows,
wayland.rs-1611-         );
wayland.rs-1612-         if let Some((oldest, newest)) = metadata {
wayland.rs-1613-             snapshot.oldest_available_scrollback_row_id = oldest;
wayland.rs-1614-             snapshot.newest_available_scrollback_row_id = newest;
wayland.rs-1609-             &snapshot.scrollback_rows,
wayland.rs-1610-             snapshot.available_scrollback_rows,
wayland.rs-1611-         );
wayland.rs-1612-         if let Some((oldest, newest)) = metadata {
wayland.rs:1613:             snapshot.oldest_available_scrollback_row_id = oldest;
wayland.rs-1614-             snapshot.newest_available_scrollback_row_id = newest;
wayland.rs-1615-         }
wayland.rs-1616-         Ok(true)
wayland.rs-1617-     }
wayland.rs-1610-             snapshot.available_scrollback_rows,
wayland.rs-1611-         );
wayland.rs-1612-         if let Some((oldest, newest)) = metadata {
wayland.rs-1613-             snapshot.oldest_available_scrollback_row_id = oldest;
wayland.rs:1614:             snapshot.newest_available_scrollback_row_id = newest;
wayland.rs-1615-         }
wayland.rs-1616-         Ok(true)
wayland.rs-1617-     }
wayland.rs-1618- 
wayland.rs-1648-                     });
wayland.rs-1649-                 let previous_rows = self
wayland.rs-1650-                     .snapshot
wayland.rs-1651-                     .as_ref()
wayland.rs:1652:                     .map_or_else(Vec::new, |current| current.scrollback_rows.clone());
wayland.rs-1653-                 self.scrollback_viewport.observe_history_change(
wayland.rs-1654-                     previous_generation,
wayland.rs-1655-                     &previous_rows,
wayland.rs-1656-                     &snapshot,
wayland.rs-1649-                 let previous_rows = self
wayland.rs-1650-                     .snapshot
wayland.rs-1651-                     .as_ref()
wayland.rs-1652-                     .map_or_else(Vec::new, |current| current.scrollback_rows.clone());
wayland.rs:1653:                 self.scrollback_viewport.observe_history_change(
wayland.rs-1654-                     previous_generation,
wayland.rs-1655-                     &previous_rows,
wayland.rs-1656-                     &snapshot,
wayland.rs-1657-                 );
wayland.rs-1679-                     .map_or(1, |snapshot| snapshot.history_generation);
wayland.rs-1680-                 let previous_rows = self
wayland.rs-1681-                     .snapshot
wayland.rs-1682-                     .as_ref()
wayland.rs:1683:                     .map_or_else(Vec::new, |snapshot| snapshot.scrollback_rows.clone());
wayland.rs-1684-                 {
wayland.rs-1685-                     let snapshot = self
wayland.rs-1686-                         .snapshot
wayland.rs-1687-                         .as_mut()
wayland.rs-1687-                         .as_mut()
wayland.rs-1688-                         .context("terminal update arrived before initial pane snapshot")?;
wayland.rs-1689-                     apply_terminal_update(snapshot, update)?;
wayland.rs-1690-                     apply_theme(snapshot, theme);
wayland.rs:1691:                     self.scrollback_viewport.observe_history_change(
wayland.rs-1692-                         previous_generation,
wayland.rs-1693-                         &previous_rows,
wayland.rs-1694-                         snapshot,
wayland.rs-1695-                     );
wayland.rs-1739-                 self.search.next_cursor = None;
wayland.rs-1740-                 self.search.pending_reveal = None;
wayland.rs-1741-                 Ok(true)
wayland.rs-1742-             }
wayland.rs:1743:             WindowUpdate::ScrollbackResyncRequired => {
wayland.rs-1744-                 self.history_page_pending = false;
wayland.rs-1745-                 self.clear_local_content_state();
wayland.rs-1746-                 Ok(true)
wayland.rs-1747-             }
wayland.rs-1744-                 self.history_page_pending = false;
wayland.rs-1745-                 self.clear_local_content_state();
wayland.rs-1746-                 Ok(true)
wayland.rs-1747-             }
wayland.rs:1748:             WindowUpdate::ScrollbackPages(pages) => self.apply_background_pages(pages),
wayland.rs-1749-             WindowUpdate::Theme(_) => Ok(false),
wayland.rs-1750-             WindowUpdate::Shutdown => {
wayland.rs-1751-                 self.controller_active = false;
wayland.rs-1752-                 self.commands = None;
wayland.rs-1797-     topology_updates: Option<Receiver<WindowTopologyUpdate>>,
wayland.rs-1798-     topology_commands: Option<Sender<WindowTopologyCommand>>,
wayland.rs-1799-     signoff: Option<SignoffProbe>,
wayland.rs-1800-     graphical_input_probe: Option<GraphicalInputProbe>,
wayland.rs:1801:     scroll_trace: bool,
wayland.rs-1802-     trusted_consent: Option<TrustedConsentUi>,
wayland.rs-1803-     cursor_style: CursorStyle,
wayland.rs-1804-     cursor_blink: bool,
wayland.rs-1805-     title_override: Option<String>,
wayland.rs-1829-     clipboard_rx: StdReceiver<ClipboardRead>,
wayland.rs-1830-     last_pointer_serial: Option<u32>,
wayland.rs-1831-     pressed_buttons: HashMap<u32, PressOwner>,
wayland.rs-1832-     vertical_wheel: WheelAccumulator,
wayland.rs:1833:     scrollback_wheel: WheelAccumulator,
wayland.rs-1834-     loop_handle: LoopHandle<'static, App>,
wayland.rs-1835-     update_waker: Waker,
wayland.rs-1836-     logical_width: u32,
wayland.rs-1837-     logical_height: u32,
wayland.rs-1969- }
wayland.rs-1970- 
wayland.rs-1971- fn terminal_update_changes_visible_content(update: &TerminalUpdate) -> bool {
wayland.rs-1972-     !update.rows.is_empty()
wayland.rs:1973:         || !update.scrolls.is_empty()
wayland.rs-1974-         || update.columns.is_some()
wayland.rs-1975-         || update.row_count.is_some()
wayland.rs-1976-         || update.palette.is_some()
wayland.rs-1977-         || update.default_colors.is_some()
wayland.rs-1975-         || update.row_count.is_some()
wayland.rs-1976-         || update.palette.is_some()
wayland.rs-1977-         || update.default_colors.is_some()
wayland.rs-1978-         || update.active_screen.is_some()
wayland.rs:1979:         || update.scrollback.is_some()
wayland.rs-1980-         || update.images.is_some()
wayland.rs-1981- }
wayland.rs-1982- 
wayland.rs-1983- fn terminal_update_full_frame_reasons(
wayland.rs-1994-                 .active_screen
wayland.rs-1995-                 .is_some_and(|active_screen| active_screen != current_active_screen),
wayland.rs-1996-         ) << 4)
wayland.rs-1997-         | (u64::from(update.images.is_some()) << 5)
wayland.rs:1998:         | (u64::from(current_has_images && !update.scrolls.is_empty()) << 6)
wayland.rs-1999- }
wayland.rs-2000- 
wayland.rs-2001- #[cfg(test)]
wayland.rs-2002- fn terminal_update_requires_full_frame(
wayland.rs-2006- ) -> bool {
wayland.rs-2007-     terminal_update_full_frame_reasons(update, current_active_screen, current_has_images) != 0
wayland.rs-2008- }
wayland.rs-2009- 
wayland.rs:2010: fn apply_scrollback_update(
wayland.rs-2011-     snapshot: &mut TerminalSnapshot,
wayland.rs-2012-     scrollback: splinterm_protocol::TerminalScrollbackUpdate,
wayland.rs-2013- ) -> Result<()> {
wayland.rs-2014-     match scrollback.transition {
wayland.rs-2008- }
wayland.rs-2009- 
wayland.rs-2010- fn apply_scrollback_update(
wayland.rs-2011-     snapshot: &mut TerminalSnapshot,
wayland.rs:2012:     scrollback: splinterm_protocol::TerminalScrollbackUpdate,
wayland.rs-2013- ) -> Result<()> {
wayland.rs-2014-     match scrollback.transition {
wayland.rs-2015-         HistoryTransition::Append { .. }
wayland.rs-2016-             if scrollback.history_generation != snapshot.history_generation =>
wayland.rs-2010- fn apply_scrollback_update(
wayland.rs-2011-     snapshot: &mut TerminalSnapshot,
wayland.rs-2012-     scrollback: splinterm_protocol::TerminalScrollbackUpdate,
wayland.rs-2013- ) -> Result<()> {
wayland.rs:2014:     match scrollback.transition {
wayland.rs-2015-         HistoryTransition::Append { .. }
wayland.rs-2016-             if scrollback.history_generation != snapshot.history_generation =>
wayland.rs-2017-         {
wayland.rs-2018-             anyhow::bail!("history append changed generation");
wayland.rs-2012-     scrollback: splinterm_protocol::TerminalScrollbackUpdate,
wayland.rs-2013- ) -> Result<()> {
wayland.rs-2014-     match scrollback.transition {
wayland.rs-2015-         HistoryTransition::Append { .. }
wayland.rs:2016:             if scrollback.history_generation != snapshot.history_generation =>
wayland.rs-2017-         {
wayland.rs-2018-             anyhow::bail!("history append changed generation");
wayland.rs-2019-         }
wayland.rs-2020-         HistoryTransition::Clear | HistoryTransition::Reflow
wayland.rs-2017-         {
wayland.rs-2018-             anyhow::bail!("history append changed generation");
wayland.rs-2019-         }
wayland.rs-2020-         HistoryTransition::Clear | HistoryTransition::Reflow
wayland.rs:2021:             if scrollback.history_generation <= snapshot.history_generation =>
wayland.rs-2022-         {
wayland.rs-2023-             anyhow::bail!("history reset did not change generation");
wayland.rs-2024-         }
wayland.rs-2025-         _ => {}
wayland.rs-2023-             anyhow::bail!("history reset did not change generation");
wayland.rs-2024-         }
wayland.rs-2025-         _ => {}
wayland.rs-2026-     }
wayland.rs:2027:     let preserve_cached = scrollback.history_generation == snapshot.history_generation
wayland.rs-2028-         && matches!(
wayland.rs-2029-             scrollback.transition,
wayland.rs-2030-             HistoryTransition::Append { .. } | HistoryTransition::Replace
wayland.rs-2031-         );
wayland.rs-2025-         _ => {}
wayland.rs-2026-     }
wayland.rs-2027-     let preserve_cached = scrollback.history_generation == snapshot.history_generation
wayland.rs-2028-         && matches!(
wayland.rs:2029:             scrollback.transition,
wayland.rs-2030-             HistoryTransition::Append { .. } | HistoryTransition::Replace
wayland.rs-2031-         );
wayland.rs-2032-     let first_returned = scrollback.rows.first().and_then(|row| row.row_id);
wayland.rs-2033-     let oldest_available = scrollback.oldest_available_row_id;
wayland.rs-2026-     }
wayland.rs-2027-     let preserve_cached = scrollback.history_generation == snapshot.history_generation
wayland.rs-2028-         && matches!(
wayland.rs-2029-             scrollback.transition,
wayland.rs:2030:             HistoryTransition::Append { .. } | HistoryTransition::Replace
wayland.rs-2031-         );
wayland.rs-2032-     let first_returned = scrollback.rows.first().and_then(|row| row.row_id);
wayland.rs-2033-     let oldest_available = scrollback.oldest_available_row_id;
wayland.rs-2034-     let mut rows = if preserve_cached {
wayland.rs-2028-         && matches!(
wayland.rs-2029-             scrollback.transition,
wayland.rs-2030-             HistoryTransition::Append { .. } | HistoryTransition::Replace
wayland.rs-2031-         );
wayland.rs:2032:     let first_returned = scrollback.rows.first().and_then(|row| row.row_id);
wayland.rs-2033-     let oldest_available = scrollback.oldest_available_row_id;
wayland.rs-2034-     let mut rows = if preserve_cached {
wayland.rs-2035-         snapshot
wayland.rs-2036-             .scrollback_rows
wayland.rs-2029-             scrollback.transition,
wayland.rs-2030-             HistoryTransition::Append { .. } | HistoryTransition::Replace
wayland.rs-2031-         );
wayland.rs-2032-     let first_returned = scrollback.rows.first().and_then(|row| row.row_id);
wayland.rs:2033:     let oldest_available = scrollback.oldest_available_row_id;
wayland.rs-2034-     let mut rows = if preserve_cached {
wayland.rs-2035-         snapshot
wayland.rs-2036-             .scrollback_rows
wayland.rs-2037-             .iter()
wayland.rs-2032-     let first_returned = scrollback.rows.first().and_then(|row| row.row_id);
wayland.rs-2033-     let oldest_available = scrollback.oldest_available_row_id;
wayland.rs-2034-     let mut rows = if preserve_cached {
wayland.rs-2035-         snapshot
wayland.rs:2036:             .scrollback_rows
wayland.rs-2037-             .iter()
wayland.rs-2038-             .filter(|row| {
wayland.rs-2039-                 row.row_id
wayland.rs-2040-                     .zip(oldest_available)
wayland.rs-2048-             .collect::<Vec<_>>()
wayland.rs-2049-     } else {
wayland.rs-2050-         Vec::new()
wayland.rs-2051-     };
wayland.rs:2052:     rows.extend(scrollback.rows);
wayland.rs-2053-     bound_history_cache(&mut rows, false);
wayland.rs-2054-     snapshot.history_generation = scrollback.history_generation;
wayland.rs-2055-     snapshot.oldest_available_scrollback_row_id = scrollback.oldest_available_row_id;
wayland.rs-2056-     snapshot.newest_available_scrollback_row_id = scrollback.newest_available_row_id;
wayland.rs-2050-         Vec::new()
wayland.rs-2051-     };
wayland.rs-2052-     rows.extend(scrollback.rows);
wayland.rs-2053-     bound_history_cache(&mut rows, false);
wayland.rs:2054:     snapshot.history_generation = scrollback.history_generation;
wayland.rs-2055-     snapshot.oldest_available_scrollback_row_id = scrollback.oldest_available_row_id;
wayland.rs-2056-     snapshot.newest_available_scrollback_row_id = scrollback.newest_available_row_id;
wayland.rs-2057-     snapshot.scrollback_rows = rows;
wayland.rs-2058-     snapshot.available_scrollback_rows = scrollback.available_rows;
wayland.rs-2051-     };
wayland.rs-2052-     rows.extend(scrollback.rows);
wayland.rs-2053-     bound_history_cache(&mut rows, false);
wayland.rs-2054-     snapshot.history_generation = scrollback.history_generation;
wayland.rs:2055:     snapshot.oldest_available_scrollback_row_id = scrollback.oldest_available_row_id;
wayland.rs-2056-     snapshot.newest_available_scrollback_row_id = scrollback.newest_available_row_id;
wayland.rs-2057-     snapshot.scrollback_rows = rows;
wayland.rs-2058-     snapshot.available_scrollback_rows = scrollback.available_rows;
wayland.rs-2059-     snapshot.omitted_oldest_scrollback_rows = snapshot
wayland.rs-2052-     rows.extend(scrollback.rows);
wayland.rs-2053-     bound_history_cache(&mut rows, false);
wayland.rs-2054-     snapshot.history_generation = scrollback.history_generation;
wayland.rs-2055-     snapshot.oldest_available_scrollback_row_id = scrollback.oldest_available_row_id;
wayland.rs:2056:     snapshot.newest_available_scrollback_row_id = scrollback.newest_available_row_id;
wayland.rs-2057-     snapshot.scrollback_rows = rows;
wayland.rs-2058-     snapshot.available_scrollback_rows = scrollback.available_rows;
wayland.rs-2059-     snapshot.omitted_oldest_scrollback_rows = snapshot
wayland.rs-2060-         .available_scrollback_rows
wayland.rs-2053-     bound_history_cache(&mut rows, false);
wayland.rs-2054-     snapshot.history_generation = scrollback.history_generation;
wayland.rs-2055-     snapshot.oldest_available_scrollback_row_id = scrollback.oldest_available_row_id;
wayland.rs-2056-     snapshot.newest_available_scrollback_row_id = scrollback.newest_available_row_id;
wayland.rs:2057:     snapshot.scrollback_rows = rows;
wayland.rs-2058-     snapshot.available_scrollback_rows = scrollback.available_rows;
wayland.rs-2059-     snapshot.omitted_oldest_scrollback_rows = snapshot
wayland.rs-2060-         .available_scrollback_rows
wayland.rs-2061-         .saturating_sub(snapshot.scrollback_rows.len());
wayland.rs-2054-     snapshot.history_generation = scrollback.history_generation;
wayland.rs-2055-     snapshot.oldest_available_scrollback_row_id = scrollback.oldest_available_row_id;
wayland.rs-2056-     snapshot.newest_available_scrollback_row_id = scrollback.newest_available_row_id;
wayland.rs-2057-     snapshot.scrollback_rows = rows;
wayland.rs:2058:     snapshot.available_scrollback_rows = scrollback.available_rows;
wayland.rs-2059-     snapshot.omitted_oldest_scrollback_rows = snapshot
wayland.rs-2060-         .available_scrollback_rows
wayland.rs-2061-         .saturating_sub(snapshot.scrollback_rows.len());
wayland.rs-2062-     Ok(())
wayland.rs-2055-     snapshot.oldest_available_scrollback_row_id = scrollback.oldest_available_row_id;
wayland.rs-2056-     snapshot.newest_available_scrollback_row_id = scrollback.newest_available_row_id;
wayland.rs-2057-     snapshot.scrollback_rows = rows;
wayland.rs-2058-     snapshot.available_scrollback_rows = scrollback.available_rows;
wayland.rs:2059:     snapshot.omitted_oldest_scrollback_rows = snapshot
wayland.rs-2060-         .available_scrollback_rows
wayland.rs-2061-         .saturating_sub(snapshot.scrollback_rows.len());
wayland.rs-2062-     Ok(())
wayland.rs-2063- }
wayland.rs-2056-     snapshot.newest_available_scrollback_row_id = scrollback.newest_available_row_id;
wayland.rs-2057-     snapshot.scrollback_rows = rows;
wayland.rs-2058-     snapshot.available_scrollback_rows = scrollback.available_rows;
wayland.rs-2059-     snapshot.omitted_oldest_scrollback_rows = snapshot
wayland.rs:2060:         .available_scrollback_rows
wayland.rs-2061-         .saturating_sub(snapshot.scrollback_rows.len());
wayland.rs-2062-     Ok(())
wayland.rs-2063- }
wayland.rs-2064- 
wayland.rs-2057-     snapshot.scrollback_rows = rows;
wayland.rs-2058-     snapshot.available_scrollback_rows = scrollback.available_rows;
wayland.rs-2059-     snapshot.omitted_oldest_scrollback_rows = snapshot
wayland.rs-2060-         .available_scrollback_rows
wayland.rs:2061:         .saturating_sub(snapshot.scrollback_rows.len());
wayland.rs-2062-     Ok(())
wayland.rs-2063- }
wayland.rs-2064- 
wayland.rs-2065- fn apply_terminal_update(snapshot: &mut TerminalSnapshot, update: TerminalUpdate) -> Result<()> {
wayland.rs-2115-     }
wayland.rs-2116-     if let Some(colors) = update.default_colors {
wayland.rs-2117-         snapshot.default_colors = colors;
wayland.rs-2118-     }
wayland.rs:2119:     if let Some(scrollback) = update.scrollback {
wayland.rs-2120-         apply_scrollback_update(snapshot, scrollback)?;
wayland.rs-2121-     }
wayland.rs-2122-     if let Some(images) = update.images {
wayland.rs-2123-         snapshot.images = Some(images);
wayland.rs-2116-     if let Some(colors) = update.default_colors {
wayland.rs-2117-         snapshot.default_colors = colors;
wayland.rs-2118-     }
wayland.rs-2119-     if let Some(scrollback) = update.scrollback {
wayland.rs:2120:         apply_scrollback_update(snapshot, scrollback)?;
wayland.rs-2121-     }
wayland.rs-2122-     if let Some(images) = update.images {
wayland.rs-2123-         snapshot.images = Some(images);
wayland.rs-2124-     }
wayland.rs-2354- }
wayland.rs-2355- 
wayland.rs-2356- fn loaded_row_position(snapshot: &TerminalSnapshot, row_id: u64) -> Option<usize> {
wayland.rs-2357-     snapshot
wayland.rs:2358:         .scrollback_rows
wayland.rs-2359-         .iter()
wayland.rs-2360-         .chain(&snapshot.visible_rows)
wayland.rs-2361-         .position(|row| row.row_id == Some(row_id))
wayland.rs-2362- }
wayland.rs-2429- 
wayland.rs-2430- fn selection_text(snapshot: &TerminalSnapshot, selection: Selection) -> Option<String> {
wayland.rs-2431-     let range = selection_range(snapshot, selection)?;
wayland.rs-2432-     let rows = snapshot
wayland.rs:2433:         .scrollback_rows
wayland.rs-2434-         .iter()
wayland.rs-2435-         .chain(&snapshot.visible_rows);
wayland.rs-2436-     let mut output = String::new();
wayland.rs-2437-     for (row_index, row) in rows
wayland.rs-2551-     }
wayland.rs-2552- }
wayland.rs-2553- 
wayland.rs-2554- fn history_overlay_status(
wayland.rs:2555:     viewport: &ScrollbackViewport,
wayland.rs-2556-     snapshot: Option<&TerminalSnapshot>,
wayland.rs-2557- ) -> Option<HistoryOverlayStatus> {
wayland.rs-2558-     let snapshot = snapshot?;
wayland.rs-2559-     (!viewport.is_live()).then_some(HistoryOverlayStatus {
wayland.rs-2557- ) -> Option<HistoryOverlayStatus> {
wayland.rs-2558-     let snapshot = snapshot?;
wayland.rs-2559-     (!viewport.is_live()).then_some(HistoryOverlayStatus {
wayland.rs-2560-         offset_from_bottom: viewport.offset_from_bottom().min(999),
wayland.rs:2561:         available_rows: snapshot.available_scrollback_rows.min(999),
wayland.rs-2562-         unseen_rows: viewport.unseen_rows().min(999),
wayland.rs-2563-     })
wayland.rs-2564- }
wayland.rs-2565- 
wayland.rs-3461-         }
wayland.rs-3462-         let Some(snapshot) = self.pane.snapshot.as_ref() else {
wayland.rs-3463-             return Ok(());
wayland.rs-3464-         };
wayland.rs:3465:         if snapshot.omitted_oldest_scrollback_rows == 0 {
wayland.rs-3466-             return Ok(());
wayland.rs-3467-         }
wayland.rs-3468-         let Some(before_row_id) = snapshot.scrollback_rows.first().and_then(|row| row.row_id)
wayland.rs-3469-         else {
wayland.rs-3464-         };
wayland.rs-3465-         if snapshot.omitted_oldest_scrollback_rows == 0 {
wayland.rs-3466-             return Ok(());
wayland.rs-3467-         }
wayland.rs:3468:         let Some(before_row_id) = snapshot.scrollback_rows.first().and_then(|row| row.row_id)
wayland.rs-3469-         else {
wayland.rs-3470-             return Ok(());
wayland.rs-3471-         };
wayland.rs-3472-         let Some(commands) = self.pane.commands.as_ref() else {
wayland.rs-3473-             return Ok(());
wayland.rs-3474-         };
wayland.rs-3475-         try_window_command(
wayland.rs-3476-             commands,
wayland.rs:3477:             WindowCommand::FetchScrollback {
wayland.rs-3478-                 splint_id: snapshot.splint_id,
wayland.rs-3479-                 incarnation: snapshot.incarnation,
wayland.rs-3480-                 terminal_revision: snapshot.revision,
wayland.rs-3481-                 history_generation: snapshot.history_generation,
wayland.rs-3485-         self.pane.history_page_pending = true;
wayland.rs-3486-         Ok(())
wayland.rs-3487-     }
wayland.rs-3488- 
wayland.rs:3489:     fn scroll_history(&mut self, action: MouseAction, lines: usize) -> Result<bool> {
wayland.rs-3490-         let snapshot = self
wayland.rs-3491-             .pane
wayland.rs-3492-             .snapshot
wayland.rs-3493-             .as_ref()
wayland.rs-3490-         let snapshot = self
wayland.rs-3491-             .pane
wayland.rs-3492-             .snapshot
wayland.rs-3493-             .as_ref()
wayland.rs:3494:             .context("scroll requires snapshot")?;
wayland.rs-3495-         let previous_offset = self.pane.scrollback_viewport.offset_from_bottom();
wayland.rs-3496-         match action {
wayland.rs-3497-             MouseAction::WheelUp => self.pane.scrollback_viewport.scroll_up(lines, snapshot),
wayland.rs-3498-             MouseAction::WheelDown => self.pane.scrollback_viewport.scroll_down(lines, snapshot),
wayland.rs-3491-             .pane
wayland.rs-3492-             .snapshot
wayland.rs-3493-             .as_ref()
wayland.rs-3494-             .context("scroll requires snapshot")?;
wayland.rs:3495:         let previous_offset = self.pane.scrollback_viewport.offset_from_bottom();
wayland.rs-3496-         match action {
wayland.rs-3497-             MouseAction::WheelUp => self.pane.scrollback_viewport.scroll_up(lines, snapshot),
wayland.rs-3498-             MouseAction::WheelDown => self.pane.scrollback_viewport.scroll_down(lines, snapshot),
wayland.rs-3499-             _ => return Ok(false),
wayland.rs-3493-             .as_ref()
wayland.rs-3494-             .context("scroll requires snapshot")?;
wayland.rs-3495-         let previous_offset = self.pane.scrollback_viewport.offset_from_bottom();
wayland.rs-3496-         match action {
wayland.rs:3497:             MouseAction::WheelUp => self.pane.scrollback_viewport.scroll_up(lines, snapshot),
wayland.rs-3498-             MouseAction::WheelDown => self.pane.scrollback_viewport.scroll_down(lines, snapshot),
wayland.rs-3499-             _ => return Ok(false),
wayland.rs-3500-         }
wayland.rs-3501-         let moved = self.pane.scrollback_viewport.offset_from_bottom() != previous_offset;
wayland.rs-3494-             .context("scroll requires snapshot")?;
wayland.rs-3495-         let previous_offset = self.pane.scrollback_viewport.offset_from_bottom();
wayland.rs-3496-         match action {
wayland.rs-3497-             MouseAction::WheelUp => self.pane.scrollback_viewport.scroll_up(lines, snapshot),
wayland.rs:3498:             MouseAction::WheelDown => self.pane.scrollback_viewport.scroll_down(lines, snapshot),
wayland.rs-3499-             _ => return Ok(false),
wayland.rs-3500-         }
wayland.rs-3501-         let moved = self.pane.scrollback_viewport.offset_from_bottom() != previous_offset;
wayland.rs-3502-         if action == MouseAction::WheelUp {
wayland.rs-3497-             MouseAction::WheelUp => self.pane.scrollback_viewport.scroll_up(lines, snapshot),
wayland.rs-3498-             MouseAction::WheelDown => self.pane.scrollback_viewport.scroll_down(lines, snapshot),
wayland.rs-3499-             _ => return Ok(false),
wayland.rs-3500-         }
wayland.rs:3501:         let moved = self.pane.scrollback_viewport.offset_from_bottom() != previous_offset;
wayland.rs-3502-         if action == MouseAction::WheelUp {
wayland.rs-3503-             let loaded = snapshot.scrollback_rows.len();
wayland.rs-3504-             let remaining =
wayland.rs-3505-                 loaded.saturating_sub(self.pane.scrollback_viewport.offset_from_bottom());
wayland.rs-3499-             _ => return Ok(false),
wayland.rs-3500-         }
wayland.rs-3501-         let moved = self.pane.scrollback_viewport.offset_from_bottom() != previous_offset;
wayland.rs-3502-         if action == MouseAction::WheelUp {
wayland.rs:3503:             let loaded = snapshot.scrollback_rows.len();
wayland.rs-3504-             let remaining =
wayland.rs-3505-                 loaded.saturating_sub(self.pane.scrollback_viewport.offset_from_bottom());
wayland.rs-3506-             let prefetch_distance = snapshot.rows.saturating_mul(2).max(32);
wayland.rs-3507-             if remaining <= prefetch_distance {
wayland.rs-3501-         let moved = self.pane.scrollback_viewport.offset_from_bottom() != previous_offset;
wayland.rs-3502-         if action == MouseAction::WheelUp {
wayland.rs-3503-             let loaded = snapshot.scrollback_rows.len();
wayland.rs-3504-             let remaining =
wayland.rs:3505:                 loaded.saturating_sub(self.pane.scrollback_viewport.offset_from_bottom());
wayland.rs-3506-             let prefetch_distance = snapshot.rows.saturating_mul(2).max(32);
wayland.rs-3507-             if remaining <= prefetch_distance {
wayland.rs-3508-                 self.request_older_history()?;
wayland.rs-3509-             }
wayland.rs-3510-         }
wayland.rs-3511-         if !moved {
wayland.rs-3512-             return Ok(false);
wayland.rs-3513-         }
wayland.rs:3514:         self.pane.scroll_started_at.get_or_insert_with(Instant::now);
wayland.rs-3515-         self.invalidate_viewport_local_state();
wayland.rs-3516-         self.refresh_ime_preedit()?;
wayland.rs-3517-         self.update_ime_cursor_rectangle();
wayland.rs-3518-         // Coalesce high-resolution wheel events until the next compositor frame.
wayland.rs-3516-         self.refresh_ime_preedit()?;
wayland.rs-3517-         self.update_ime_cursor_rectangle();
wayland.rs-3518-         // Coalesce high-resolution wheel events until the next compositor frame.
wayland.rs-3519-         // Re-shaping the entire viewport synchronously for every axis event made
wayland.rs:3520:         // fast scrolling stall the Wayland dispatch loop.
wayland.rs-3521-         self.pane.viewport_dirty = true;
wayland.rs-3522-         Ok(true)
wayland.rs-3523-     }
wayland.rs-3524- 
wayland.rs-3554-             return Ok(());
wayland.rs-3555-         };
wayland.rs-3556-         match probe.step {
wayland.rs-3557-             SignoffStep::WaitHistory => {
wayland.rs:3558:                 if snapshot.available_scrollback_rows >= 5_000 {
wayland.rs-3559-                     probe.step = SignoffStep::LoadSelectionWindow;
wayland.rs-3560-                 }
wayland.rs-3561-             }
wayland.rs-3562-             SignoffStep::LoadSelectionWindow => {
wayland.rs-3559-                     probe.step = SignoffStep::LoadSelectionWindow;
wayland.rs-3560-                 }
wayland.rs-3561-             }
wayland.rs-3562-             SignoffStep::LoadSelectionWindow => {
wayland.rs:3563:                 self.scroll_history(MouseAction::WheelUp, usize::MAX)?;
wayland.rs-3564-                 if self
wayland.rs-3565-                     .pane
wayland.rs-3566-                     .snapshot
wayland.rs-3567-                     .as_ref()
wayland.rs-3564-                 if self
wayland.rs-3565-                     .pane
wayland.rs-3566-                     .snapshot
wayland.rs-3567-                     .as_ref()
wayland.rs:3568:                     .is_some_and(|snapshot| snapshot.scrollback_rows.len() >= 640)
wayland.rs-3569-                 {
wayland.rs-3570-                     probe.step = SignoffStep::BeginSelection;
wayland.rs-3571-                 }
wayland.rs-3572-             }
wayland.rs-3570-                     probe.step = SignoffStep::BeginSelection;
wayland.rs-3571-                 }
wayland.rs-3572-             }
wayland.rs-3573-             SignoffStep::LoadClientCache => {
wayland.rs:3574:                 self.scroll_history(MouseAction::WheelUp, usize::MAX)?;
wayland.rs-3575-                 let snapshot = self.pane.snapshot.as_ref().context("sign-off snapshot")?;
wayland.rs-3576-                 let cache_bytes = history_cache_bytes(&snapshot.scrollback_rows);
wayland.rs-3577-                 let loaded_rows = snapshot.scrollback_rows.len();
wayland.rs-3578-                 let first_row_id = snapshot.scrollback_rows.first().and_then(|row| row.row_id);
wayland.rs-3572-             }
wayland.rs-3573-             SignoffStep::LoadClientCache => {
wayland.rs-3574-                 self.scroll_history(MouseAction::WheelUp, usize::MAX)?;
wayland.rs-3575-                 let snapshot = self.pane.snapshot.as_ref().context("sign-off snapshot")?;
wayland.rs:3576:                 let cache_bytes = history_cache_bytes(&snapshot.scrollback_rows);
wayland.rs-3577-                 let loaded_rows = snapshot.scrollback_rows.len();
wayland.rs-3578-                 let first_row_id = snapshot.scrollback_rows.first().and_then(|row| row.row_id);
wayland.rs-3579-                 let bounded_eviction_observed = probe.cache_window.zip(first_row_id).is_some_and(
wayland.rs-3580-                     |((previous_rows, previous_first), current_first)| {
wayland.rs-3573-             SignoffStep::LoadClientCache => {
wayland.rs-3574-                 self.scroll_history(MouseAction::WheelUp, usize::MAX)?;
wayland.rs-3575-                 let snapshot = self.pane.snapshot.as_ref().context("sign-off snapshot")?;
wayland.rs-3576-                 let cache_bytes = history_cache_bytes(&snapshot.scrollback_rows);
wayland.rs:3577:                 let loaded_rows = snapshot.scrollback_rows.len();
wayland.rs-3578-                 let first_row_id = snapshot.scrollback_rows.first().and_then(|row| row.row_id);
wayland.rs-3579-                 let bounded_eviction_observed = probe.cache_window.zip(first_row_id).is_some_and(
wayland.rs-3580-                     |((previous_rows, previous_first), current_first)| {
wayland.rs-3581-                         loaded_rows <= previous_rows && current_first < previous_first
wayland.rs-3574-                 self.scroll_history(MouseAction::WheelUp, usize::MAX)?;
wayland.rs-3575-                 let snapshot = self.pane.snapshot.as_ref().context("sign-off snapshot")?;
wayland.rs-3576-                 let cache_bytes = history_cache_bytes(&snapshot.scrollback_rows);
wayland.rs-3577-                 let loaded_rows = snapshot.scrollback_rows.len();
wayland.rs:3578:                 let first_row_id = snapshot.scrollback_rows.first().and_then(|row| row.row_id);
wayland.rs-3579-                 let bounded_eviction_observed = probe.cache_window.zip(first_row_id).is_some_and(
wayland.rs-3580-                     |((previous_rows, previous_first), current_first)| {
wayland.rs-3581-                         loaded_rows <= previous_rows && current_first < previous_first
wayland.rs-3582-                     },
wayland.rs-3586-                 }
wayland.rs-3587-                 let row_capacity_hit = loaded_rows >= MAX_CACHED_HISTORY_ROWS;
wayland.rs-3588-                 let byte_capacity_hit = cache_bytes >= MAX_CACHED_HISTORY_BYTES;
wayland.rs-3589-                 if (row_capacity_hit || byte_capacity_hit || bounded_eviction_observed)
wayland.rs:3590:                     && self.pane.scrollback_viewport.offset_from_bottom()
wayland.rs-3591-                         >= snapshot.scrollback_rows.len().saturating_sub(snapshot.rows)
wayland.rs-3592-                 {
wayland.rs-3593-                     anyhow::ensure!(
wayland.rs-3594-                         snapshot.scrollback_rows.len() <= MAX_CACHED_HISTORY_ROWS
wayland.rs-3587-                 let row_capacity_hit = loaded_rows >= MAX_CACHED_HISTORY_ROWS;
wayland.rs-3588-                 let byte_capacity_hit = cache_bytes >= MAX_CACHED_HISTORY_BYTES;
wayland.rs-3589-                 if (row_capacity_hit || byte_capacity_hit || bounded_eviction_observed)
wayland.rs-3590-                     && self.pane.scrollback_viewport.offset_from_bottom()
wayland.rs:3591:                         >= snapshot.scrollback_rows.len().saturating_sub(snapshot.rows)
wayland.rs-3592-                 {
wayland.rs-3593-                     anyhow::ensure!(
wayland.rs-3594-                         snapshot.scrollback_rows.len() <= MAX_CACHED_HISTORY_ROWS
[100 matches limit reached. Use limit=200 for more, or refine pattern. 50.0KB limit reached]
candidate-binaries.sha256
graphical/control-candidate/process-attribution.json
graphical/control-candidate/progress.json
graphical/control-candidate/__pycache__/run.cpython-314.pyc
graphical/control-candidate/run.log
graphical/control-candidate/run.py
graphical/control-candidate/summary.json
graphical/smoke/preflight.json
graphical/smoke/splinterm-retention.json
provenance/candidate-tracked.patch
provenance/git-status.txt
provenance/identities.json
provenance/toolchain.txt
provenance/untracked-source-files.txt
provenance/untracked-source.tar
SHA256SUMS
summary.json
summary.md
validation/cargo-test-workspace-serial.log
read: crates/splinterd/src/main.rs
read: crates/splinterm/src/wayland.rs
read: crates/splinterm/src/wayland.rs
read: docs/benchmarks/artifacts/2026-07-27-plan0011-scroll-boun...
read: docs/benchmarks/artifacts/2026-07-27-plan0011-scroll-boun...
read: docs/benchmarks/artifacts/2026-07-27-plan0011-scroll-boun...
        _ => internal(),
    }
}
#[allow(
    clippy::too_many_lines,
    reason = "wire conversion keeps one revision's bounded semantic damage atomic"
)]
fn visible_row_changed(
    previous: &[splinterd::LiveRow],
    current: &[splinterd::LiveRow],
    index: usize,
) -> bool {
    previous.get(index) != current.get(index)
}
fn wire_scrollback_update(
    rows: &[splinterd::LiveRow],
    scrollback: splinterm_terminal::ScrollbackSnapshot,
    previous_history: HistoryState,
    reflow: bool,
    appended_rows: usize,
) -> TerminalScrollbackUpdate {
    let transition = if scrollback.history_generation != previous_history.generation {
        if reflow {
            HistoryTransition::Reflow
        } else if scrollback.available_rows == 0 {
            HistoryTransition::Clear
        } else {
            HistoryTransition::Replace
        }
    } else if appended_rows > 0 && appended_rows <= usize::from(MAX_ROWS) {
        HistoryTransition::Append {
            appended_rows,
            trimmed_rows: previous_history
                .available_rows
                .saturating_add(appended_rows)
                .saturating_sub(scrollback.available_rows),
        }
    } else {
        HistoryTransition::Replace
    };
    let maximum_rows = match transition {
        HistoryTransition::Append { appended_rows, .. } => {
            appended_rows.min(MAX_SNAPSHOT_SCROLLBACK_ROWS)
        }
        HistoryTransition::Clear | HistoryTransition::Reflow | HistoryTransition::Replace => {
            MAX_SNAPSHOT_SCROLLBACK_ROWS
        }
    };
    let first = rows.len().saturating_sub(maximum_rows);
    let rows: Vec<_> = rows[first..].iter().cloned().map(wire_row).collect();
    TerminalScrollbackUpdate {
        transition,
        history_generation: scrollback.history_generation,
        oldest_available_row_id: scrollback.oldest_available_row_id,
        newest_available_row_id: scrollback.newest_available_row_id,
        omitted_oldest_rows: scrollback.available_rows.saturating_sub(rows.len()),
        available_rows: scrollback.available_rows,
        rows,
    }
}
fn bound_wire_scrolls(scrolls: &mut Vec<TerminalScroll>, damaged: &mut [bool]) {
    if scrolls.len() > MAX_UPDATE_SCROLLS {
        // Scroll operations are an optimization over the authoritative final rows. A
        // coalesced burst can contain more scroll damage records than one wire update
        // permits, so fall back to bounded final-state viewport patches rather than
        // widening the protocol limit or emitting a semantically incomplete prefix.
        scrolls.clear();
        damaged.fill(true);
    }
}
fn wire_update(
    updates: &[TerminalUpdate],
    snapshot: &LiveSnapshot,
    previous_revision: u64,
    previous_history: HistoryState,
    previous_visible_rows: &[splinterd::LiveRow],
    include_images: bool,
) -> Result<WireTerminalUpdate, ProtocolError> {
    let mut damaged = vec![false; snapshot.visible_rows.len()];
    let mut scrolls = Vec::new();
    let mut cursor = false;
    let mut title = false;
    let mut modes = false;
    let mut palette = false;
    let mut dimensions = false;
    let mut scrollback = false;
    let mut images = false;
    let mut reflow = false;
    let mut appended_rows = 0_usize;
    for damage in updates.iter().flat_map(TerminalUpdate::damage) {
        match damage {
            TerminalDamage::FullSnapshot => {
                damaged.fill(true);
                cursor = true;
                title = true;
                modes = true;
                palette = true;
                dimensions = true;
                scrollback = true;
                images = true;
            }
            TerminalDamage::Viewport => damaged.fill(true),
            TerminalDamage::Rows { start, end } => {
                for item in damaged.iter_mut().take(*end).skip(*start) {
                    *item = true;
                }
            }
            TerminalDamage::Scroll {
                direction,
                region,
                rows,
            } => {
                let start = usize::try_from(region.start()).map_err(|_| internal())?;
                let end = usize::try_from(region.end()).map_err(|_| internal())?;
                if *direction == ScrollDirection::Forward
                    && start == 0
                    && end == snapshot.dimensions.rows
                    && snapshot.active_screen == ActiveScreen::Normal
                {
                    appended_rows = appended_rows.saturating_add(*rows);
                }
                for item in damaged.iter_mut().take(end).skip(start) {
                    *item = true;
                }
                scrolls.push(TerminalScroll {
                    direction: match direction {
                        ScrollDirection::Forward => WireScrollDirection::Forward,
                        ScrollDirection::Reverse => WireScrollDirection::Reverse,
                    },
                    start_row: start,
                    end_row: end,
                    rows: *rows,
                });
            }
            TerminalDamage::Cursor { .. } => cursor = true,
            TerminalDamage::Modes => modes = true,
            TerminalDamage::Dimensions => {
                dimensions = true;
                reflow = true;
                damaged.fill(true);
            }
            TerminalDamage::Title => title = true,
            TerminalDamage::Palette { .. } => palette = true,
            TerminalDamage::Scrollback => scrollback = true,
            TerminalDamage::Images { .. } => {
                damaged.fill(true);
                images = true;
            }
        }
    }
    bound_wire_scrolls(&mut scrolls, &mut damaged);
    let wire_scrollback = if scrollback {
        Some(wire_scrollback_update(
            &snapshot.scrollback_rows,
            snapshot.scrollback,
            previous_history,
            reflow,
            appended_rows,
        ))
    } else {
        None
    };
    let position = snapshot.cursor.cursor.position();
    let rows = damaged
        .into_iter()
        .enumerate()
        .filter(|(index, changed)| {
            *changed && visible_row_changed(previous_visible_rows, &snapshot.visible_rows, *index)
        })
        .map(|(index, _)| TerminalRowPatch {
            index,
            row: wire_row(snapshot.visible_rows[index].clone()),
        })
        .collect();
    Ok(WireTerminalUpdate {
        base_revision: previous_revision,
        revision: updates.last().ok_or_else(internal)?.revision().value(),
        rows,
        scrolls,
        cursor: cursor.then_some(TerminalCursor {
            column: position.column,
            row: position.row,
            deferred_wrap: snapshot.cursor.cursor.deferred_wrap(),
        }),
        title: title.then(|| snapshot.title.clone()),
        input_modes: modes.then_some(wire_modes(snapshot.modes)),
        active_screen: modes.then_some(wire_active_screen(snapshot.active_screen)),
        palette: palette.then(|| snapshot.palette.to_vec()),
        default_colors: palette.then_some(snapshot.default_colors),
        columns: dimensions.then_some(snapshot.dimensions.columns),
        row_count: dimensions.then_some(snapshot.dimensions.rows),
        scrollback: wire_scrollback,
        images: (include_images && images).then(|| Box::new(wire_image_plane(snapshot))),
    })
}
fn wire_snapshot(snapshot: LiveSnapshot, include_images: bool) -> TerminalSnapshot {
    let position = snapshot.cursor.cursor.position();
    let exited_code = snapshot.exited.and_then(|status| status.code);
    let images = include_images.then(|| Box::new(wire_image_plane(&snapshot)));
    let exited_signal = snapshot.exited.and_then(|status| status.signal);
    TerminalSnapshot {
        splint_id: snapshot.splint_id,
        incarnation: snapshot.incarnation.value(),
        revision: snapshot.revision.value(),
        columns: snapshot.dimensions.columns,
        rows: snapshot.dimensions.rows,
        cursor_column: position.column,
        cursor_row: position.row,
        cursor_deferred_wrap: snapshot.cursor.cursor.deferred_wrap(),
        active_screen: wire_active_screen(snapshot.active_screen),
        input_modes: wire_modes(snapshot.modes),
        palette: snapshot.palette.to_vec(),
        default_colors: snapshot.default_colors,
        title: snapshot.title,
        visible_rows: snapshot.visible_rows.into_iter().map(wire_row).collect(),
        history_generation: snapshot.scrollback.history_generation,
        oldest_available_scrollback_row_id: snapshot.scrollback.oldest_available_row_id,
        newest_available_scrollback_row_id: snapshot.scrollback.newest_available_row_id,
        scrollback_rows: snapshot.scrollback_rows.into_iter().map(wire_row).collect(),
        available_scrollback_rows: snapshot.scrollback.available_rows,
        omitted_oldest_scrollback_rows: snapshot.scrollback.omitted_oldest_rows,
        images,
        exited_code,
        exited_signal,
    }
}
fn wire_image_plane(snapshot: &LiveSnapshot) -> splinterm_protocol::TerminalImagePlane {
    use splinterm_protocol::{
        ImageAlphaMode as WireAlphaMode, ImageContentMetadata as WireContent,
        ImageErasePolicy as WireErasePolicy, ImagePixelRect, ImagePixelSize,
        ImagePlacement as WirePlacement, ImageRetention as WireRetention,
        ImageSourceFormat as WireSourceFormat,
    };
    let contents = snapshot
        .image_contents
        .iter()
        .map(|content| WireContent {
            content_id: content.id.value(),
            generation: content.generation,
            width: content.width,
            height: content.height,
            source_format: match content.source_format {
                splinterm_terminal::ImageSourceFormat::Sixel => WireSourceFormat::Sixel,
                splinterm_terminal::ImageSourceFormat::KittyRgb => WireSourceFormat::KittyRgb,
                splinterm_terminal::ImageSourceFormat::KittyRgba => WireSourceFormat::KittyRgba,
                splinterm_terminal::ImageSourceFormat::KittyPng => WireSourceFormat::KittyPng,
                splinterm_terminal::ImageSourceFormat::Iterm2 => WireSourceFormat::Iterm2,
            },
            alpha_mode: match content.alpha_mode {
                splinterm_terminal::ImageAlphaMode::Opaque => WireAlphaMode::Opaque,
                splinterm_terminal::ImageAlphaMode::Premultiplied => WireAlphaMode::Premultiplied,
            },
            digest: content.digest,
            byte_length: content.byte_charge,
            retention: match content.retention {
                splinterm_terminal::ImageRetention::WhilePlaced => WireRetention::WhilePlaced,
                splinterm_terminal::ImageRetention::ExplicitDelete => WireRetention::ExplicitDelete,
            },
        })
        .collect();
    let placements = snapshot
        .image_placements
        .iter()
        .map(|placement| WirePlacement {
            placement_id: placement.id.value(),
            content_id: placement.content_id.value(),
            row_id: placement.row_id,
            column: placement.column,
            source: ImagePixelRect {
                x: placement.source.x,
                y: placement.source.y,
                width: placement.source.width,
                height: placement.source.height,
            },
            destination_columns: placement.destination.columns,
            destination_rows: placement.destination.rows,
            source_cell_size: placement.source_cell_size.map(|size| ImagePixelSize {
                width: size.width,
                height: size.height,
            }),
            x_offset: placement.x_offset,
            y_offset: placement.y_offset,
            z_index: placement.z_index,
            application_image_id: placement.application_image_id,
            application_placement_id: placement.application_placement_id,
            creation_order: placement.creation_order,
            erase_policy: match placement.erase_policy {
                splinterm_terminal::ImageErasePolicy::TextOverwrite => {
                    WireErasePolicy::TextOverwrite
                }
                splinterm_terminal::ImageErasePolicy::ExplicitDelete => {
                    WireErasePolicy::ExplicitDelete
                }
            },
        })
        .collect();
    splinterm_protocol::TerminalImagePlane {
        screen: wire_active_screen(snapshot.active_screen),
        contents,
        placements,
    }
}
fn wire_active_screen(screen: ActiveScreen) -> WireActiveScreen {
    match screen {
        ActiveScreen::Normal => WireActiveScreen::Normal,
        ActiveScreen::Alternate => WireActiveScreen::Alternate,
    }
}
fn wire_modes(modes: splinterm_terminal::TerminalModes) -> TerminalInputModes {
    TerminalInputModes {
        application_cursor: modes.application_cursor,
        application_keypad: modes.application_keypad,
        focus_reporting: modes.focus_reporting,
        bracketed_paste: modes.bracketed_paste,
        cursor_visible: modes.cursor_visible,
        cursor_blink: modes.cursor_blink,
        mouse_tracking: match modes.mouse_tracking {
            splinterm_terminal::MouseTracking::None => WireMouseTracking::None,
            splinterm_terminal::MouseTracking::Normal => WireMouseTracking::Normal,
            splinterm_terminal::MouseTracking::Button => WireMouseTracking::Button,
            splinterm_terminal::MouseTracking::Any => WireMouseTracking::Any,
        },
        sgr_mouse: modes.sgr_mouse,
    }
}
fn wire_row(row: splinterd::LiveRow) -> TerminalRow {
    TerminalRow {
        row_id: row.row_id,
        linebreak: row.linebreak,
        cells: row
            .cells
            .into_iter()
            .map(|cell| TerminalCell {
                content: cell.content,
                spacer_remaining: cell.spacer_remaining,
                attributes: CellAttributes {
                    bold: cell.attributes.bold,
                    dim: cell.attributes.dim,
                    italic: cell.attributes.italic,
                    underline: match cell.attributes.underline {
                        splinterm_terminal::UnderlineStyle::None => WireUnderlineStyle::None,
                        splinterm_terminal::UnderlineStyle::Single => WireUnderlineStyle::Single,
                        splinterm_terminal::UnderlineStyle::Double => WireUnderlineStyle::Double,
                        splinterm_terminal::UnderlineStyle::Curly => WireUnderlineStyle::Curly,
                        splinterm_terminal::UnderlineStyle::Dotted => WireUnderlineStyle::Dotted,
                        splinterm_terminal::UnderlineStyle::Dashed => WireUnderlineStyle::Dashed,
                    },
                    underline_color_source: wire_color_source(
                        cell.attributes.underline_color.source(),
                    ),
                    underline_color: cell.attributes.underline_color.value(),
                    strikethrough: cell.attributes.strikethrough,
                    blink: cell.attributes.blink,
                    conceal: cell.attributes.conceal,
                    reverse: cell.attributes.reverse,
                    foreground_source: wire_color_source(cell.attributes.foreground.source()),
                    foreground: cell.attributes.foreground.value(),
                    background_source: wire_color_source(cell.attributes.background.source()),
                    background: cell.attributes.background.value(),
                },
            })
            .collect(),
    }
}
fn wire_color_source(source: TerminalColorSource) -> ColorSource {
    match source {
        TerminalColorSource::Default => ColorSource::Default,
        TerminalColorSource::Base16 => ColorSource::Base16,
        TerminalColorSource::Base256 => ColorSource::Base256,
        TerminalColorSource::Rgb => ColorSource::Rgb,
    }
}
async fn verify_peer(stream: &UnixStream) -> Result<(PeerIdentity, Option<consent::PeerMonitor>)> {
    let mut identity = PeerIdentity::from_stream(stream)?;
    if identity.uid != rustix::process::geteuid().as_raw() {
        bail!("peer uid mismatch");
    }
    let monitor = match consent::PeerMonitor::initialize(stream, identity.pid).await {
        Ok((monitor, executable)) => {
            identity.install_persistent_executable(executable);
            Some(monitor)
        }
        Err(error) => {
            warn!(%error, "persistent peer identity unavailable; policy authorization disabled");
            None
        }
    };
    Ok((identity, monitor))
}
async fn prepare_socket_parent(path: &Path) -> Result<()> {
    let parent = path.parent().context("socket path has no parent")?;
    fs::create_dir_all(parent).await?;
    let metadata = fs::symlink_metadata(parent).await?;
    if !metadata.is_dir() || metadata.uid() != rustix::process::geteuid().as_raw() {
        bail!("unsafe socket directory owner or type");
    }
    fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700)).await?;
    Ok(())
}
async fn verify_socket(path: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(path).await?;
    if !metadata.file_type().is_socket()
        || metadata.uid() != rustix::process::geteuid().as_raw()
        || metadata.mode() & 0o777 != 0o600
    {
        bail!("unsafe socket metadata");
    }
    Ok(())
}
async fn remove_stale_socket(path: &Path) -> Result<()> {
    match UnixStream::connect(path).await {
        Ok(_) => bail!("splinterd is already running at {}", path.display()),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        Err(error) if error.kind() == ErrorKind::ConnectionRefused => {
            let metadata = fs::symlink_metadata(path).await?;
            if !metadata.file_type().is_socket()
                || metadata.uid() != rustix::process::geteuid().as_raw()
            {
                bail!("refusing to remove unsafe stale endpoint");
            }
            fs::remove_file(path).await?;
            Ok(())
        }
        Err(error) => Err(error.into()),
    }
}
fn socket_path() -> Result<PathBuf> {
    if let Some(path) = env::var_os("SPLINTERM_SOCKET") {
        return Ok(path.into());
    }
    let runtime = env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .context("XDG_RUNTIME_DIR is unset; set SPLINTERM_SOCKET explicitly")?;
    Ok(runtime.join("splinterm/splinterd.sock"))
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};
    #[test]
    fn public_live_row_materializes_unchanged_wire_content() {
        let attributes = splinterm_terminal::Attributes::default().into();
        let row = splinterd::LiveRow {
            row_id: Some(7),
            linebreak: true,
            cells: vec![splinterd::LiveCell {
                content: "A".to_owned(),
                spacer_remaining: None,
                attributes,
            }],
        };
        let expected = TerminalRow {
            row_id: Some(7),
            linebreak: true,
            cells: vec![TerminalCell {
                content: "A".to_owned(),
                spacer_remaining: None,
                attributes: CellAttributes::default(),
            }],
        };
        let actual = wire_row(row);
        assert_eq!(actual, expected);
        assert_eq!(
            serde_json::to_vec(&actual).unwrap(),
            serde_json::to_vec(&expected).unwrap()
        );
    }
    #[test]
    fn oversized_append_batch_falls_back_to_bounded_history_replace() {
        let attributes = splinterm_terminal::Attributes::default().into();
        let rows = (1..=20)
            .map(|row_id| splinterd::LiveRow {
                row_id: Some(row_id),
                linebreak: true,
                cells: vec![splinterd::LiveCell {
                    content: format!("row-{row_id}"),
                    spacer_remaining: None,
                    attributes,
                }],
            })
            .collect::<Vec<_>>();
        let scrollback = splinterm_terminal::ScrollbackSnapshot {
            history_generation: 7,
            oldest_available_row_id: Some(1),
            newest_available_row_id: Some(20),
            available_rows: 20,
            returned_rows: 20,
            omitted_oldest_rows: 0,
        };
        let previous = HistoryState {
            revision: 10,
            generation: 7,
            available_rows: 0,
        };
        let update = wire_scrollback_update(
            &rows,
            scrollback,
            previous,
            false,
            usize::from(MAX_ROWS) + 1,
        );
        assert_eq!(update.transition, HistoryTransition::Replace);
        assert_eq!(update.rows.len(), MAX_SNAPSHOT_SCROLLBACK_ROWS);
        assert_eq!(update.omitted_oldest_rows, 4);
        let wire = WireTerminalUpdate {
            base_revision: 10,
            revision: 20,
            rows: Vec::new(),
            scrolls: Vec::new(),
            cursor: None,
            title: None,
            input_modes: None,
            active_screen: None,
            palette: None,
            default_colors: None,
            columns: None,
            row_count: None,
            scrollback: Some(update),
            images: None,
        };
        wire.validate_against(10, 7, 80, 24).unwrap();
    }
    #[test]
    fn oversized_scroll_batch_falls_back_to_bounded_viewport_patches() {
        let scroll = TerminalScroll {
            direction: WireScrollDirection::Forward,
            start_row: 0,
            end_row: 2,
            rows: 1,
        };
        let mut scrolls = vec![scroll; MAX_UPDATE_SCROLLS + 1];
        let mut damaged = vec![false; MAX_ROWS as usize];
        bound_wire_scrolls(&mut scrolls, &mut damaged);
        assert!(scrolls.is_empty());
        assert!(damaged.iter().all(|damaged| *damaged));
        assert!(damaged.len() <= splinterm_protocol::MAX_UPDATE_ROW_PATCHES);
    }
    #[test]
    fn append_delta_history_is_wire_identical_to_full_materialization() {
        let attributes = splinterm_terminal::Attributes::default().into();
        let make_row = |row_id| splinterd::LiveRow {
            row_id: Some(row_id),
            linebreak: true,
            cells: vec![splinterd::LiveCell {
                content: format!("row-{row_id}"),
                spacer_remaining: None,
                attributes,
            }],
        };
        let full_rows = (1..=5).map(make_row).collect::<Vec<_>>();
        let delta_rows = full_rows[3..].to_vec();
        let scrollback = splinterm_terminal::ScrollbackSnapshot {
            history_generation: 7,
            oldest_available_row_id: Some(1),
            newest_available_row_id: Some(5),
            available_rows: 5,
            returned_rows: 5,
            omitted_oldest_rows: 0,
        };
        let previous = HistoryState {
            revision: 10,
            generation: 7,
            available_rows: 3,
        };
        let full = wire_scrollback_update(&full_rows, scrollback, previous, false, 2);
        let delta = wire_scrollback_update(&delta_rows, scrollback, previous, false, 2);
        assert_eq!(full, delta);
        assert_eq!(
            serde_json::to_vec(&full).unwrap(),
            serde_json::to_vec(&delta).unwrap()
        );
        assert_eq!(full.rows.len(), 2);
        assert_eq!(full.omitted_oldest_rows, 3);
        assert_eq!(
            full.transition,
            HistoryTransition::Append {
                appended_rows: 2,
                trimmed_rows: 0,
            }
        );
    }
    #[tokio::test]
    async fn binary_image_content_channel_is_raw_windowed_and_acknowledged() {
        use splinterm_terminal::{
            ImageAlphaMode, ImagePlane, ImageRetention, ImageSourceFormat, NewImageContent,
        };
        let mut plane = ImagePlane::default();
        let content_id = plane
            .insert_content(
                ActiveScreen::Normal,
                NewImageContent {
                    width: 1,
                    height: 1,
                    source_format: ImageSourceFormat::Sixel,
                    alpha_mode: ImageAlphaMode::Opaque,
                    pixels: &[1, 2, 3, 255],
[1104 more lines in file. Use offset=6120 to continue.]
#[cfg(test)]
fn terminal_update_requires_full_frame(
    update: &TerminalUpdate,
    current_active_screen: ActiveScreen,
    current_has_images: bool,
) -> bool {
    terminal_update_full_frame_reasons(update, current_active_screen, current_has_images) != 0
}
fn apply_scrollback_update(
    snapshot: &mut TerminalSnapshot,
    scrollback: splinterm_protocol::TerminalScrollbackUpdate,
) -> Result<()> {
    match scrollback.transition {
        HistoryTransition::Append { .. }
            if scrollback.history_generation != snapshot.history_generation =>
        {
            anyhow::bail!("history append changed generation");
        }
        HistoryTransition::Clear | HistoryTransition::Reflow
            if scrollback.history_generation <= snapshot.history_generation =>
        {
            anyhow::bail!("history reset did not change generation");
        }
        _ => {}
    }
    let preserve_cached = scrollback.history_generation == snapshot.history_generation
        && matches!(
            scrollback.transition,
            HistoryTransition::Append { .. } | HistoryTransition::Replace
        );
    let first_returned = scrollback.rows.first().and_then(|row| row.row_id);
    let oldest_available = scrollback.oldest_available_row_id;
    let mut rows = if preserve_cached {
        snapshot
            .scrollback_rows
            .iter()
            .filter(|row| {
                row.row_id
                    .zip(oldest_available)
                    .is_some_and(|(id, oldest)| id >= oldest)
                    && row
                        .row_id
                        .zip(first_returned)
                        .is_some_and(|(id, first)| id < first)
            })
            .cloned()
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };
    rows.extend(scrollback.rows);
    bound_history_cache(&mut rows, false);
    snapshot.history_generation = scrollback.history_generation;
    snapshot.oldest_available_scrollback_row_id = scrollback.oldest_available_row_id;
    snapshot.newest_available_scrollback_row_id = scrollback.newest_available_row_id;
    snapshot.scrollback_rows = rows;
    snapshot.available_scrollback_rows = scrollback.available_rows;
    snapshot.omitted_oldest_scrollback_rows = snapshot
        .available_scrollback_rows
        .saturating_sub(snapshot.scrollback_rows.len());
    Ok(())
}
fn apply_terminal_update(snapshot: &mut TerminalSnapshot, update: TerminalUpdate) -> Result<()> {
    update
        .validate_against(
            snapshot.revision,
            snapshot.history_generation,
            snapshot.columns,
            snapshot.rows,
        )
        .map_err(|error| anyhow::anyhow!(error.message))?;
    if let Some(columns) = update.columns {
        if columns == 0 || columns > usize::from(splinterm_protocol::MAX_COLUMNS) {
            anyhow::bail!("terminal update columns exceed protocol limits");
        }
        snapshot.columns = columns;
    }
    if let Some(rows) = update.row_count {
        if rows == 0 || rows > usize::from(splinterm_protocol::MAX_ROWS) {
            anyhow::bail!("terminal update rows exceed protocol limits");
        }
        snapshot.rows = rows;
        snapshot
            .visible_rows
            .resize_with(rows, || blank_row(snapshot.columns));
        snapshot.visible_rows.truncate(rows);
    }
    for patch in update.rows {
        if patch.index >= snapshot.rows || patch.row.cells.len() > snapshot.columns {
            anyhow::bail!("terminal row patch exceeds current dimensions");
        }
        snapshot.visible_rows[patch.index] = patch.row;
    }
    if let Some(cursor) = update.cursor {
        snapshot.cursor_column = cursor.column;
        snapshot.cursor_row = cursor.row;
        snapshot.cursor_deferred_wrap = cursor.deferred_wrap;
[6680 more lines in file. Use offset=2100 to continue.]
                transition: HistoryTransition::Append {
                    appended_rows: 2,
                    trimmed_rows: 2,
                },
                history_generation: 1,
                oldest_available_row_id: Some(3),
                newest_available_row_id: Some(6),
                rows: vec![history_row(5, 0), history_row(6, 0)],
                available_rows: 4,
                omitted_oldest_rows: 2,
            },
        )
        .unwrap();
        assert_eq!(
            current
                .scrollback_rows
                .iter()
                .filter_map(|row| row.row_id)
                .collect::<Vec<_>>(),
            vec![3, 4, 5, 6]
        );
        assert_eq!(current.omitted_oldest_scrollback_rows, 0);
    }
    #[test]
    fn bounded_history_replace_applies_visible_marker_without_resync() {
        let mut current = snapshot(SplintId::new(), 7, 10);
        current.columns = 1;
        current.rows = 1;
        current.visible_rows = vec![blank_row(1)];
        current.scrollback_rows = (1..=4).map(|id| history_row(id, 0)).collect();
        current.available_scrollback_rows = 4;
        current.oldest_available_scrollback_row_id = Some(1);
        current.newest_available_scrollback_row_id = Some(4);
        let marker = TerminalRow {
            row_id: Some(21),
            linebreak: true,
            cells: vec![TerminalCell {
                content: "SPLINTERBENCH_DONE".into(),
                spacer_remaining: None,
                attributes: current.visible_rows[0].cells[0].attributes,
            }],
        };
        apply_terminal_update(
            &mut current,
            TerminalUpdate {
                base_revision: 10,
                revision: 11,
                rows: vec![splinterm_protocol::TerminalRowPatch {
                    index: 0,
                    row: marker.clone(),
                }],
                scrolls: Vec::new(),
                cursor: None,
                title: None,
                input_modes: None,
                active_screen: None,
                palette: None,
                default_colors: None,
                columns: None,
                row_count: None,
                scrollback: Some(splinterm_protocol::TerminalScrollbackUpdate {
                    transition: HistoryTransition::Replace,
                    history_generation: 1,
                    oldest_available_row_id: Some(1),
                    newest_available_row_id: Some(20),
                    rows: vec![history_row(19, 0), history_row(20, 0)],
                    available_rows: 20,
                    omitted_oldest_rows: 18,
                }),
                images: None,
            },
        )
        .expect("bounded replace update");
        assert_eq!(current.revision, 11);
        assert_eq!(current.visible_rows[0], marker);
        assert_eq!(current.newest_available_scrollback_row_id, Some(20));
    }
    #[test]
    fn semantic_update_applies_exact_row_cursor_and_title_revision() {
        let mut current = snapshot(SplintId::new(), 7, 10);
        current.columns = 2;
        current.rows = 1;
        current.visible_rows = vec![blank_row(2)];
        let row = TerminalRow {
            row_id: Some(8),
            linebreak: true,
            cells: vec![TerminalCell {
                content: "x".into(),
                spacer_remaining: None,
                attributes: current.visible_rows[0].cells[0].attributes,
            }],
        };
        apply_terminal_update(
            &mut current,
            TerminalUpdate {
                base_revision: 10,
                revision: 11,
                rows: vec![splinterm_protocol::TerminalRowPatch {
                    index: 0,
                    row: row.clone(),
                }],
                scrolls: Vec::new(),
                cursor: Some(splinterm_protocol::TerminalCursor {
                    column: 1,
                    row: 0,
                    deferred_wrap: true,
                }),
                title: Some("revision eleven".into()),
                input_modes: None,
                active_screen: None,
                palette: None,
                default_colors: None,
                columns: None,
                row_count: None,
                scrollback: Some(splinterm_protocol::TerminalScrollbackUpdate {
                    transition: splinterm_protocol::HistoryTransition::Reflow,
                    history_generation: 2,
                    oldest_available_row_id: Some(7),
                    newest_available_row_id: Some(7),
                    rows: vec![TerminalRow {
                        row_id: Some(7),
                        linebreak: true,
                        cells: Vec::new(),
                    }],
                    available_rows: 1,
[1000 more lines in file. Use offset=7780 to continue.]
{
  "isolation": {
    "cleanup_verified": true,
    "monitor": "DP-2",
    "no_initial_focus": true,
    "workspace": 8
  },
  "launch_mode": "prestarted_daemon_client_launch",
  "memory_timeline": {
    "baseline": {
      "aggregate": {
        "private_anon_bytes": 16855040,
        "private_file_bytes": 7458816,
        "pss_bytes": 27613184,
        "rss_bytes": 47734784,
        "shared_bytes": 23420928,
        "shmem_bytes": 0
      },
      "processes": [
        {
          "name": "splinterd",
          "pid": 1441158,
          "private_anon_bytes": 2048000,
          "private_file_bytes": 3141632,
          "pss_bytes": 5218304,
          "rss_bytes": 7880704,
          "shared_bytes": 2691072,
          "shmem_bytes": 0
        },
        {
          "name": "splinterm",
          "pid": 1441167,
          "private_anon_bytes": 4595712,
          "private_file_bytes": 4304896,
          "pss_bytes": 11438080,
          "rss_bytes": 19668992,
          "shared_bytes": 10768384,
          "shmem_bytes": 0
        },
        {
          "name": "python",
          "pid": 1441172,
          "private_anon_bytes": 10211328,
          "private_file_bytes": 12288,
          "pss_bytes": 10956800,
          "rss_bytes": 20185088,
          "shared_bytes": 9961472,
          "shmem_bytes": 0
        }
      ]
    },
    "classification": {
      "private_anon": "min(total private, Anonymous) from smaps_rollup",
      "private_file": "total private minus classified private anonymous",
      "shared": "Shared_Clean plus Shared_Dirty",
      "shmem": "ShmemPmdMapped"
    },
    "marker_visible": {
      "aggregate": {
        "private_anon_bytes": 63913984,
        "private_file_bytes": 8179712,
        "pss_bytes": 80022528,
        "rss_bytes": 129134592,
        "shared_bytes": 57040896,
        "shmem_bytes": 0
      },
      "processes": [
        {
          "name": "splinterd",
          "pid": 1441158,
          "private_anon_bytes": 24117248,
          "private_file_bytes": 3141632,
          "pss_bytes": 27286528,
          "rss_bytes": 29949952,
          "shared_bytes": 2691072,
          "shmem_bytes": 0
        },
        {
          "name": "splinterm",
          "pid": 1441167,
          "private_anon_bytes": 29585408,
          "private_file_bytes": 5025792,
          "pss_bytes": 41781248,
          "rss_bytes": 78999552,
          "shared_bytes": 44388352,
          "shmem_bytes": 0
        },
        {
          "name": "python",
          "pid": 1441172,
          "private_anon_bytes": 10211328,
          "private_file_bytes": 12288,
          "pss_bytes": 10954752,
          "rss_bytes": 20185088,
          "shared_bytes": 9961472,
          "shmem_bytes": 0
        }
      ]
    },
    "peak_observed_rss_bytes": 129134592,
    "settles": [
      {
        "memory": {
          "aggregate": {
            "private_anon_bytes": 63913984,
            "private_file_bytes": 8179712,
            "pss_bytes": 80026624,
            "rss_bytes": 129134592,
            "shared_bytes": 57040896,
            "shmem_bytes": 0
          },
          "processes": [
            {
              "name": "splinterd",
              "pid": 1441158,
              "private_anon_bytes": 24117248,
              "private_file_bytes": 3141632,
              "pss_bytes": 27287552,
              "rss_bytes": 29949952,
              "shared_bytes": 2691072,
              "shmem_bytes": 0
            },
            {
              "name": "splinterm",
              "pid": 1441167,
              "private_anon_bytes": 29585408,
              "private_file_bytes": 5025792,
              "pss_bytes": 41782272,
              "rss_bytes": 78999552,
              "shared_bytes": 44388352,
              "shmem_bytes": 0
            },
            {
              "name": "python",
              "pid": 1441172,
              "private_anon_bytes": 10211328,
              "private_file_bytes": 12288,
              "pss_bytes": 10956800,
              "rss_bytes": 20185088,
              "shared_bytes": 9961472,
              "shmem_bytes": 0
            }
          ]
        },
        "seconds": 2.0
      }
    ]
  },
  "notes": [],
  "processes": {
    "child_included": true,
    "child_pid": 1441172,
    "count": 3,
    "root_pids": [
      1441158,
      1441167
    ]
  },
  "retention": {
    "clear_cycles": 9,
    "context_switches": 137,
    "cpu_ticks": 66,
    "lines": 5000,
    "retained_growth_bytes": 81399808,
    "rss_baseline_bytes": 47734784,
    "rss_peak_observed_bytes": 129134592,
    "rss_post_settle_bytes": 129134592,
    "trigger_to_visible_marker_ns": 586072142
  },
  "schema": "splinterm.benchmark.graphical-retention.v2",
  "terminal": "splinterm",
  "valid": true,
  "variant": "bounded-replace-smoke"
}
{
  "candidate_reduction_percent": -10.78458552813973,
  "improvement_established": false,
  "improvement_gate_percent": 40.0,
  "samples_per_variant": 10,
  "schema": "splinterm.plan0011.graphical-control-candidate.v3",
  "seed": 1106,
  "summary": {
    "candidate": {
      "cpu_ticks": {
        "maximum": 100,
        "median": 76.5,
        "minimum": 51
      },
      "retained_growth_bytes": {
        "maximum": 84709376,
        "median": 81838080.0,
        "minimum": 76361728
      },
      "rss_post_settle_bytes": {
        "maximum": 132468736,
        "median": 129685504.0,
        "minimum": 124203008
      },
      "trigger_to_visible_marker_ns": {
        "maximum": 859438368,
        "median": 615855954.5,
        "minimum": 595342569
      }
    },
    "control": {
      "cpu_ticks": {
        "maximum": 22,
        "median": 19.0,
        "minimum": 18
      },
      "retained_growth_bytes": {
        "maximum": 73953280,
        "median": 73871360.0,
        "minimum": 73641984
      },
      "rss_post_settle_bytes": {
        "maximum": 121655296,
        "median": 121552896.0,
        "minimum": 121331712
      },
      "trigger_to_visible_marker_ns": {
        "maximum": 431458167,
        "median": 396125124.0,
        "minimum": 372977064
      }
    }
  },
  "valid": true,
  "warmups_per_variant": 2
}
{
  "candidate": {
    "python": {
      "max_mib": 0.0,
      "median_mib": 0.0,
      "min_mib": 0.0,
      "samples": 10
    },
    "splinterd": {
      "max_mib": 21.45703125,
      "median_mib": 21.171875,
      "min_mib": 20.40625,
      "samples": 10
    },
    "splinterm": {
      "max_mib": 59.5859375,
      "median_mib": 56.8359375,
      "min_mib": 52.41796875,
      "samples": 10
    }
  },
  "control": {
    "python": {
      "max_mib": 0.0,
      "median_mib": 0.0,
      "min_mib": 0.0,
      "samples": 10
    },
    "splinterd": {
      "max_mib": 34.30078125,
      "median_mib": 34.109375,
      "min_mib": 33.9140625,
      "samples": 10
    },
    "splinterm": {
      "max_mib": 36.4296875,
      "median_mib": 36.310546875,
      "min_mib": 36.11328125,
      "samples": 10
    }
  }
}
read: docs/benchmarks/artifacts/2026-07-27-plan0011-scroll-boun...
read: docs/benchmarks/artifacts/2026-07-27-plan0011-scroll-boun...
read: docs/benchmarks/artifacts/2026-07-27-plan0011-scroll-boun...
read: docs/benchmarks/artifacts/2026-07-27-plan0011-scroll-boun...
read: docs/benchmarks/artifacts/2026-07-27-plan0011-scroll-boun...
read: docs/benchmarks/artifacts/2026-07-27-plan0011-scroll-boun...
{
  "attempts": [
    {
      "failure": null,
      "finished_ns": 1785186523309111048,
      "iteration": 0,
      "notes": [],
      "output": "raw/warmup/00/control",
      "phase": "warmup",
      "result_exists": true,
      "returncode": 0,
      "started_ns": 1785186519547205703,
      "status": "finished",
      "valid": true,
      "variant": "control"
    },
    {
      "failure": null,
      "finished_ns": 1785186527086132112,
      "iteration": 1,
      "notes": [],
      "output": "raw/warmup/01/control",
      "phase": "warmup",
      "result_exists": true,
      "returncode": 0,
      "started_ns": 1785186523309508733,
      "status": "finished",
      "valid": true,
      "variant": "control"
    },
    {
      "failure": null,
      "finished_ns": 1785186531257170377,
      "iteration": 0,
      "notes": [],
      "output": "raw/warmup/00/candidate",
      "phase": "warmup",
      "result_exists": true,
      "returncode": 0,
      "started_ns": 1785186527086433763,
      "status": "finished",
      "valid": true,
      "variant": "candidate"
    },
    {
      "failure": null,
      "finished_ns": 1785186535348745712,
      "iteration": 1,
      "notes": [],
      "output": "raw/warmup/01/candidate",
      "phase": "warmup",
      "result_exists": true,
      "returncode": 0,
      "started_ns": 1785186531257511978,
      "status": "finished",
      "valid": true,
      "variant": "candidate"
    },
    {
      "failure": null,
      "finished_ns": 1785186539342997851,
      "iteration": 5,
      "notes": [],
      "output": "raw/measured/05/candidate",
      "phase": "measured",
      "result_exists": true,
      "returncode": 0,
      "started_ns": 1785186535349127612,
      "status": "finished",
      "valid": true,
      "variant": "candidate"
    },
    {
      "failure": null,
      "finished_ns": 1785186543107146593,
      "iteration": 4,
      "notes": [],
      "output": "raw/measured/04/control",
      "phase": "measured",
      "result_exists": true,
      "returncode": 0,
      "started_ns": 1785186539343399657,
      "status": "finished",
      "valid": true,
      "variant": "control"
    },
    {
      "failure": null,
      "finished_ns": 1785186547095842870,
      "iteration": 4,
      "notes": [],
      "output": "raw/measured/04/candidate",
      "phase": "measured",
      "result_exists": true,
      "returncode": 0,
      "started_ns": 1785186543107502792,
      "status": "finished",
      "valid": true,
      "variant": "candidate"
    },
    {
      "failure": null,
      "finished_ns": 1785186550874072327,
      "iteration": 9,
      "notes": [],
      "output": "raw/measured/09/control",
      "phase": "measured",
      "result_exists": true,
      "returncode": 0,
      "started_ns": 1785186547096223094,
      "status": "finished",
      "valid": true,
      "variant": "control"
    },
    {
      "failure": null,
      "finished_ns": 1785186555059611109,
      "iteration": 2,
      "notes": [],
      "output": "raw/measured/02/candidate",
      "phase": "measured",
      "result_exists": true,
      "returncode": 0,
      "started_ns": 1785186550874492082,
      "status": "finished",
      "valid": true,
      "variant": "candidate"
    },
    {
      "failure": null,
      "finished_ns": 1785186559243923656,
      "iteration": 3,
      "notes": [],
      "output": "raw/measured/03/candidate",
      "phase": "measured",
      "result_exists": true,
      "returncode": 0,
      "started_ns": 1785186555059967028,
      "status": "finished",
      "valid": true,
      "variant": "candidate"
    },
    {
      "failure": null,
      "finished_ns": 1785186562995623529,
      "iteration": 5,
      "notes": [],
      "output": "raw/measured/05/control",
      "phase": "measured",
      "result_exists": true,
      "returncode": 0,
      "started_ns": 1785186559244275734,
      "status": "finished",
      "valid": true,
      "variant": "control"
    },
    {
      "failure": null,
      "finished_ns": 1785186567174438951,
      "iteration": 1,
      "notes": [],
      "output": "raw/measured/01/candidate",
      "phase": "measured",
      "result_exists": true,
      "returncode": 0,
      "started_ns": 1785186562996034693,
      "status": "finished",
      "valid": true,
      "variant": "candidate"
    },
    {
      "failure": null,
      "finished_ns": 1785186570950460933,
      "iteration": 6,
      "notes": [],
      "output": "raw/measured/06/control",
      "phase": "measured",
      "result_exists": true,
      "returncode": 0,
      "started_ns": 1785186567174908783,
      "status": "finished",
      "valid": true,
      "variant": "control"
    },
    {
      "failure": null,
      "finished_ns": 1785186574756220152,
      "iteration": 8,
      "notes": [],
      "output": "raw/measured/08/control",
      "phase": "measured",
      "result_exists": true,
      "returncode": 0,
      "started_ns": 1785186570950874822,
      "status": "finished",
      "valid": true,
      "variant": "control"
    },
    {
      "failure": null,
      "finished_ns": 1785186578533685427,
      "iteration": 2,
      "notes": [],
      "output": "raw/measured/02/control",
      "phase": "measured",
      "result_exists": true,
      "returncode": 0,
      "started_ns": 1785186574756644586,
      "status": "finished",
      "valid": true,
      "variant": "control"
    },
    {
      "failure": null,
      "finished_ns": 1785186582293129516,
      "iteration": 1,
      "notes": [],
      "output": "raw/measured/01/control",
      "phase": "measured",
      "result_exists": true,
      "returncode": 0,
      "started_ns": 1785186578534119710,
      "status": "finished",
      "valid": true,
      "variant": "control"
    },
    {
      "failure": null,
      "finished_ns": 1785186586091998312,
      "iteration": 7,
      "notes": [],
      "output": "raw/measured/07/control",
      "phase": "measured",
      "result_exists": true,
      "returncode": 0,
      "started_ns": 1785186582293742875,
      "status": "finished",
      "valid": true,
      "variant": "control"
    },
    {
      "failure": null,
      "finished_ns": 1785186590076982605,
      "iteration": 6,
      "notes": [],
      "output": "raw/measured/06/candidate",
      "phase": "measured",
      "result_exists": true,
      "returncode": 0,
      "started_ns": 1785186586092391318,
      "status": "finished",
      "valid": true,
      "variant": "candidate"
    },
    {
      "failure": null,
      "finished_ns": 1785186594053493427,
      "iteration": 9,
      "notes": [],
      "output": "raw/measured/09/candidate",
      "phase": "measured",
      "result_exists": true,
      "returncode": 0,
      "started_ns": 1785186590077451250,
      "status": "finished",
      "valid": true,
      "variant": "candidate"
    },
    {
      "failure": null,
      "finished_ns": 1785186598281618512,
      "iteration": 8,
      "notes": [],
      "output": "raw/measured/08/candidate",
      "phase": "measured",
      "result_exists": true,
      "returncode": 0,
      "started_ns": 1785186594053970523,
      "status": "finished",
      "valid": true,
      "variant": "candidate"
    },
    {
      "failure": null,
      "finished_ns": 1785186602061741001,
      "iteration": 0,
      "notes": [],
      "output": "raw/measured/00/control",
      "phase": "measured",
      "result_exists": true,
      "returncode": 0,
      "started_ns": 1785186598282040712,
      "status": "finished",
      "valid": true,
      "variant": "control"
    },
    {
      "failure": null,
      "finished_ns": 1785186605831146671,
      "iteration": 3,
      "notes": [],
      "output": "raw/measured/03/control",
      "phase": "measured",
      "result_exists": true,
      "returncode": 0,
      "started_ns": 1785186602062208808,
      "status": "finished",
      "valid": true,
      "variant": "control"
    },
    {
      "failure": null,
      "finished_ns": 1785186609809358497,
      "iteration": 7,
      "notes": [],
      "output": "raw/measured/07/candidate",
      "phase": "measured",
      "result_exists": true,
      "returncode": 0,
      "started_ns": 1785186605831542889,
      "status": "finished",
      "valid": true,
      "variant": "candidate"
    },
    {
      "failure": null,
      "finished_ns": 1785186613774661697,
      "iteration": 0,
      "notes": [],
      "output": "raw/measured/00/candidate",
      "phase": "measured",
      "result_exists": true,
      "returncode": 0,
      "started_ns": 1785186609809778462,
      "status": "finished",
      "valid": true,
      "variant": "candidate"
    }
  ],
  "schema": "splinterm.plan0011.matrix-progress.v1",
  "seed": 1106,
  "status": "complete"
}
{
  "candidate_reduction_percent": -10.78458552813973,
  "improvement_established": false,
  "improvement_gate_percent": 40.0,
  "samples_per_variant": 10,
  "schema": "splinterm.plan0011.graphical-control-candidate.v3",
  "seed": 1106,
  "summary": {
    "candidate": {
      "cpu_ticks": {
        "maximum": 100,
        "median": 76.5,
        "minimum": 51
      },
      "retained_growth_bytes": {
        "maximum": 84709376,
        "median": 81838080.0,
        "minimum": 76361728
      },
      "rss_post_settle_bytes": {
        "maximum": 132468736,
        "median": 129685504.0,
        "minimum": 124203008
      },
      "trigger_to_visible_marker_ns": {
        "maximum": 859438368,
        "median": 615855954.5,
        "minimum": 595342569
      }
    },
    "control": {
      "cpu_ticks": {
        "maximum": 22,
        "median": 19.0,
        "minimum": 18
      },
      "retained_growth_bytes": {
        "maximum": 73953280,
        "median": 73871360.0,
        "minimum": 73641984
      },
      "rss_post_settle_bytes": {
        "maximum": 121655296,
        "median": 121552896.0,
        "minimum": 121331712
      },
      "trigger_to_visible_marker_ns": {
        "maximum": 431458167,
        "median": 396125124.0,
        "minimum": 372977064
      }
    }
  },
  "valid": true,
  "warmups_per_variant": 2
}
#!/usr/bin/env python3
import json, os, pathlib, random, statistics, subprocess, sys, time
HERE = pathlib.Path(__file__).resolve()
ROOT = next((p for p in HERE.parents if (p/'Cargo.toml').is_file() and (p/'tools/benchmark/run-graphical-retention-v2.py').is_file()), None)
if ROOT is None: raise RuntimeError('repository root not found')
OUT=HERE.parent; RUNNER=ROOT/'tools/benchmark/run-graphical-retention-v2.py'; SEED=1106; WARMUPS=2; SAMPLES=10
BINARIES={'control':pathlib.Path('/tmp/splinterm-plan0011-final-control-target/release'),'candidate':pathlib.Path('/tmp/splinterm-plan0011-final-candidate-bin')}
state={'schema':'splinterm.plan0011.matrix-progress.v1','seed':SEED,'status':'running','attempts':[]}
def save(): (OUT/'progress.json').write_text(json.dumps(state,indent=2,sort_keys=True)+'\n')
def fail(msg): state.update(status='aborted',failure=msg); save(); raise RuntimeError(msg)
for variant,base in BINARIES.items():
 missing=[n for n in ('splinterm','splinterd','splinterm-pty-child') if not (base/n).is_file()]
 if missing: fail(f'{variant} missing binaries: {missing}')
rng=random.Random(SEED)
for phase,count in (('warmup',WARMUPS),('measured',SAMPLES)):
 cases=[(v,i) for i in range(count) for v in BINARIES]; rng.shuffle(cases)
 for variant,iteration in cases:
  directory=OUT/'raw'/phase/f'{iteration:02d}'/variant; directory.mkdir(parents=True,exist_ok=True)
  attempt={'phase':phase,'iteration':iteration,'variant':variant,'status':'running','started_ns':time.time_ns(),'output':str(directory.relative_to(OUT))}; state['attempts'].append(attempt); save()
  base=BINARIES[variant]; env=os.environ.copy(); env.update(SPLINTERBENCH_SPLINTERM_CLIENT=str(base/'splinterm'),SPLINTERBENCH_SPLINTERM_DAEMON=str(base/'splinterd'),SPLINTERM_PTY_HELPER=str(base/'splinterm-pty-child'))
  cp=subprocess.run([sys.executable,str(RUNNER),str(directory),'--terminal','splinterm','--variant',variant,'--lines','5000','--settle-points','2'],cwd=ROOT,env=env,text=True,capture_output=True,timeout=60)
  (directory/'runner.stdout').write_text(cp.stdout); (directory/'runner.stderr').write_text(cp.stderr); path=directory/'splinterm-retention.json'
  attempt.update(status='finished',returncode=cp.returncode,finished_ns=time.time_ns(),result_exists=path.exists())
  if not path.exists(): save(); fail(f'{variant} {phase} {iteration} produced no result')
  result=json.loads(path.read_text()); attempt.update(valid=result.get('valid'),notes=result.get('notes',[]),failure=result.get('failure')); save()
  if cp.returncode or not result.get('valid') or not result.get('isolation',{}).get('cleanup_verified'): fail(f'{variant} {phase} {iteration} failed guard')
summary={}
for variant in BINARIES:
 results=[json.loads(p.read_text()) for p in (OUT/'raw'/'measured').glob(f'*/{variant}/splinterm-retention.json')]
 if len(results)!=SAMPLES or any(not d.get('valid') for d in results): fail(f'{variant} measured set incomplete')
 summary[variant]={}
 for key in ('retained_growth_bytes','rss_post_settle_bytes','trigger_to_visible_marker_ns','cpu_ticks'):
  vals=[d['retention'][key] for d in results]; summary[variant][key]={'median':statistics.median(vals),'minimum':min(vals),'maximum':max(vals)}
control=summary['control']['retained_growth_bytes']['median']; candidate=summary['candidate']['retained_growth_bytes']['median']
report={'schema':'splinterm.plan0011.graphical-control-candidate.v3','seed':SEED,'warmups_per_variant':WARMUPS,'samples_per_variant':SAMPLES,'summary':summary,'candidate_reduction_percent':(control-candidate)*100/control,'improvement_gate_percent':40.0,'improvement_established':candidate<=control*.60,'valid':True}
(OUT/'summary.json').write_text(json.dumps(report,indent=2,sort_keys=True)+'\n'); state['status']='complete'; save(); print(json.dumps(report,indent=2,sort_keys=True))
{
  "workspace": 8,
  "monitor": "DP-2",
  "inactive": true,
  "user_workspace_untouched": true
}
4462444e78acdf61b8a1f2e4b9ae8f7bc031987d016a6879a95cace80cdf7cd2  candidate-binaries.sha256
1e3f384a1af4176ca6ae1af95136c6b301c7f28921cff46e8378a595ceff5598  graphical/control-candidate/__pycache__/run.cpython-314.pyc
4224122893b3280ede4caee46fa97f1444b848072a985844bb1f8aca2a489d69  graphical/control-candidate/process-attribution.json
24b6de4c0e139a9a4e71dbb4b299f972d2b1d4698462048045a10ec8d65962f8  graphical/control-candidate/progress.json
e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855  graphical/control-candidate/raw/measured/00/candidate/runner.stderr
86943d7cb26591496cd2b16516c2fa00537aceedfc7c6fb67c32fc972d254887  graphical/control-candidate/raw/measured/00/candidate/runner.stdout
dd28a33a60801f08d33882af875011d8c00bf230234a26e7ab64d1de7aae507d  graphical/control-candidate/raw/measured/00/candidate/splinterm-retention.json
e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855  graphical/control-candidate/raw/measured/00/control/runner.stderr
10097c483e12dee3cf7844c4c626dff0218d3b27659c091d4efb229646228d3e  graphical/control-candidate/raw/measured/00/control/runner.stdout
673df4acef27fae5624bfd8eaa196229d515299e5a0e6c1d72f7bcd1c59204f9  graphical/control-candidate/raw/measured/00/control/splinterm-retention.json
e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855  graphical/control-candidate/raw/measured/01/candidate/runner.stderr
8b5867ff9af42e11a388cd858fb581b09a2fae8ba7e5808da2de3972c5107384  graphical/control-candidate/raw/measured/01/candidate/runner.stdout
84a4080591ddcbccf465ccf7524f9296badce9b38dcd1bf6242f008c2a0507e9  graphical/control-candidate/raw/measured/01/candidate/splinterm-retention.json
e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855  graphical/control-candidate/raw/measured/01/control/runner.stderr
cd72adb629a1c8b96d7b827062431b1cb93b8199e397ab1cda5dcc142645158c  graphical/control-candidate/raw/measured/01/control/runner.stdout
936f9cbe0ec3ca7e2df4bbcad0c5fcc2d41e20a8d4d4c8316879b39e5e272ff2  graphical/control-candidate/raw/measured/01/control/splinterm-retention.json
e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855  graphical/control-candidate/raw/measured/02/candidate/runner.stderr
d69d84565ae3ddbd41dc5406e0894802af95bdef7896b3162da832fddce63262  graphical/control-candidate/raw/measured/02/candidate/runner.stdout
cdbf8f9a6f2764b4f6bf8dab2818fa74ba753e372f0396bd79e825734fc24105  graphical/control-candidate/raw/measured/02/candidate/splinterm-retention.json
e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855  graphical/control-candidate/raw/measured/02/control/runner.stderr
0a04e37cb4ab596c241e3255001861afa32ca811b2f1002b01a56d500ecc468e  graphical/control-candidate/raw/measured/02/control/runner.stdout
366e6bc9d3a85a39ce290aaa5b7311b327732aa9db75401a4277dc077da60b7e  graphical/control-candidate/raw/measured/02/control/splinterm-retention.json
e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855  graphical/control-candidate/raw/measured/03/candidate/runner.stderr
51a48c06fd69fc2546713c0f6c1ff1c9ef519420f396f7303ae9e11a61381a33  graphical/control-candidate/raw/measured/03/candidate/runner.stdout
af4faa912d37bcc2e63d10ec4a91635f98e8bb57e7b4c75f5a9c68a5d908f622  graphical/control-candidate/raw/measured/03/candidate/splinterm-retention.json
e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855  graphical/control-candidate/raw/measured/03/control/runner.stderr
25e4e93a2f30af53d691ac8d0873c1927ef83fd5048c033e86b33e2a0f40d725  graphical/control-candidate/raw/measured/03/control/runner.stdout
b38af6236feab45817a0d7e8c5d4e86026449619f9b61aafad29f057d6e76b14  graphical/control-candidate/raw/measured/03/control/splinterm-retention.json
e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855  graphical/control-candidate/raw/measured/04/candidate/runner.stderr
012550df0349e84f84c5dda94a8f9fa4db48b5ddc4b3a9ee039944b9bad62cef  graphical/control-candidate/raw/measured/04/candidate/runner.stdout
1f564fbe7afe552493b850f0fc29ff39156788c90a2dea86dd745ff878078ef4  graphical/control-candidate/raw/measured/04/candidate/splinterm-retention.json
e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855  graphical/control-candidate/raw/measured/04/control/runner.stderr
c6d76526236059c571d24745d06c25ebdfc89d87b73c56c33eb2383a65982f11  graphical/control-candidate/raw/measured/04/control/runner.stdout
8eeb63835afbc992ef9fdf4c7636a1d61265077d7f40acebd6c039cefee1f6fa  graphical/control-candidate/raw/measured/04/control/splinterm-retention.json
e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855  graphical/control-candidate/raw/measured/05/candidate/runner.stderr
299679b3c3a26dd24244f8a2fc4b5e068c47566eb2cb25d0831595597c2baf6f  graphical/control-candidate/raw/measured/05/candidate/runner.stdout
b232cc9d0c6be11667a8e43fc30e79f85b0fbafef671e1c2ad50e58d6673fa76  graphical/control-candidate/raw/measured/05/candidate/splinterm-retention.json
e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855  graphical/control-candidate/raw/measured/05/control/runner.stderr
8a16a9cd554410f3def1d5068a3a7f7090ea1780e3fc677f38ff343f5c4a190b  graphical/control-candidate/raw/measured/05/control/runner.stdout
c807edc55bfff8d6ef46f072e1b424ea22516fd02145e088f39dde1f9f1f9934  graphical/control-candidate/raw/measured/05/control/splinterm-retention.json
e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855  graphical/control-candidate/raw/measured/06/candidate/runner.stderr
efa8e79ab0b9718684fd0fa546f61992b327bf2aab6df2d10e66746d26199ea6  graphical/control-candidate/raw/measured/06/candidate/runner.stdout
550d6ece4f0402407a9a2ea8831bdaa5837845e39ae959140513d9509a10df23  graphical/control-candidate/raw/measured/06/candidate/splinterm-retention.json
e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855  graphical/control-candidate/raw/measured/06/control/runner.stderr
39d59ad08132263098f1a425531159ec4b87783c92e8a82956310db0502dea2e  graphical/control-candidate/raw/measured/06/control/runner.stdout
67b48096e34751009b3d8cab87973a261e2d52ce33f369b3b918f9ac84e76b04  graphical/control-candidate/raw/measured/06/control/splinterm-retention.json
e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855  graphical/control-candidate/raw/measured/07/candidate/runner.stderr
c7f807b4d943c8581614e0a127561c1db3a83ae0298b4c45db8adf7857eda353  graphical/control-candidate/raw/measured/07/candidate/runner.stdout
46b4f686aa8506bc02d381c0365a1ee5f4cf6c13e586814c378daf4a1b812d88  graphical/control-candidate/raw/measured/07/candidate/splinterm-retention.json
e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855  graphical/control-candidate/raw/measured/07/control/runner.stderr
77e597a2bf4014caca0a92c88351e276bb97a1e35fbdb65ee6564a322ca9f55c  graphical/control-candidate/raw/measured/07/control/runner.stdout
cebfb7fdf4c1be873428af2b1f76abfcb1cc3af7ecfea40c8527a01c504b8c32  graphical/control-candidate/raw/measured/07/control/splinterm-retention.json
e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855  graphical/control-candidate/raw/measured/08/candidate/runner.stderr
2bf31b872cac8a16fc0ccfc83e909746446287bc75c9cc4f435c25095524c7c4  graphical/control-candidate/raw/measured/08/candidate/runner.stdout
84945e1246f7a7871fb78c5e9621500c5c19d0a97c5a7eec470e0834aa72284d  graphical/control-candidate/raw/measured/08/candidate/splinterm-retention.json
e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855  graphical/control-candidate/raw/measured/08/control/runner.stderr
4a5f874d9d1b16c583dca3d5e03195af5844b2b2b053c317bff520cd42920bb1  graphical/control-candidate/raw/measured/08/control/runner.stdout
1933f887cd321889dd35ea4123cfeda8a7e0e804cfb8ec7f7c2c06e2455e09ad  graphical/control-candidate/raw/measured/08/control/splinterm-retention.json
e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855  graphical/control-candidate/raw/measured/09/candidate/runner.stderr
fbbaaece5d48e0206bbda9f024c7529e993df528d06819ca41acda87467eac2b  graphical/control-candidate/raw/measured/09/candidate/runner.stdout
62b14b15737380cd263f3dd0ee08c83aa740bd23dbe33adb231f36979c2f897e  graphical/control-candidate/raw/measured/09/candidate/splinterm-retention.json
e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855  graphical/control-candidate/raw/measured/09/control/runner.stderr
53d9280f78267a3d5d0fadda401a6fe1e74fbe2dca40335d1be52b8697b3925b  graphical/control-candidate/raw/measured/09/control/runner.stdout
e994aa12d1b1c55de923c85d6f3a4353322087ced54c11fde13f73ab769e04d3  graphical/control-candidate/raw/measured/09/control/splinterm-retention.json
e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855  graphical/control-candidate/raw/warmup/00/candidate/runner.stderr
7fd527bb2c42453043172ba04e2b1f09c6faf8a08d2c7b6c7494d6009b54c32b  graphical/control-candidate/raw/warmup/00/candidate/runner.stdout
5260de80cd226350d1a2f7a920581a5e383b0f30149155917aff603a614c63ca  graphical/control-candidate/raw/warmup/00/candidate/splinterm-retention.json
e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855  graphical/control-candidate/raw/warmup/00/control/runner.stderr
0f528f6040e24e5d990a62072b62c6992f55b7b78b37007766fdbc0fb3c62def  graphical/control-candidate/raw/warmup/00/control/runner.stdout
b20786ba6ec937511688c4b99195bc37fcff36c22ae2aaa4fd5fa336ce649229  graphical/control-candidate/raw/warmup/00/control/splinterm-retention.json
e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855  graphical/control-candidate/raw/warmup/01/candidate/runner.stderr
cabdb9d1c8c4e6be4c0219e68d2d5791d53f102e5a1b27fb8204a565206d26e3  graphical/control-candidate/raw/warmup/01/candidate/runner.stdout
68ab734f412f32bbe0bb2047ddc8b7f26eedfe9f2cb3068d07d8adbef6514312  graphical/control-candidate/raw/warmup/01/candidate/splinterm-retention.json
e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855  graphical/control-candidate/raw/warmup/01/control/runner.stderr
faf03207e822e15ae490b1f3781d5fe60c785c38666d705bfad700e95485a516  graphical/control-candidate/raw/warmup/01/control/runner.stdout
492c3b77aff46258400a898bdd7ab3eb0ca149af6f2ffe4e9875bb1bfc3a0b29  graphical/control-candidate/raw/warmup/01/control/splinterm-retention.json
9be323849c423202621a7eec9d5483f9cf79fc2c9884c91c272bac76f154f265  graphical/control-candidate/run.log
712b1ca44db60204a4b090b830fd7d5b5e6b0eb89dc549504a0cf7d207b90996  graphical/control-candidate/run.py
9be323849c423202621a7eec9d5483f9cf79fc2c9884c91c272bac76f154f265  graphical/control-candidate/summary.json
1a5bbf5187117fb9c84480e0aa94fda0a6d18b183479ec2193702c1608e2cfc3  graphical/smoke/preflight.json
38d16abfefcb8c5920262640913b41c846b9f4001cb04748b2d84f10fa0f953f  graphical/smoke/splinterm-retention.json
ef4db184f8c64cfde545386cc1409a6e47b519c6221099fafac9a3fb7bce5540  provenance/candidate-tracked.patch
e3b3a8fe4f308a31a6aaa8598596f27784abda41fe560d27ef298ce65b2eaa1e  provenance/git-status.txt
9fde8829311a5a819a1d3d430572256d84f8578ac989d46e257345d039d14dc7  provenance/identities.json
e1c3585d0e28540db4d6f1d007f43030c138932a0fbad25a63a2259511dc7240  provenance/toolchain.txt
28866e8e40fee06bebb12d3c161a381a61cefb922c89e6495a7b8fca19635269  provenance/untracked-source-files.txt
c4101929edca15514bd4f2162253e5a5632b1e1cb02da54e6757b7a415329fb4  provenance/untracked-source.tar
6372de52c011584a862b5036a33dcbb05d3789986bd6c40aac544ed2991e5433  summary.json
0851defbbcbc166716966095fc056ec7fcc6a532b0132da05548c78c547c5abf  summary.md
a36d7e3976a7b19b10536c0947628b741005f88cfa253ff33c822ae294f38b79  validation/cargo-test-workspace-serial.log
crates/splinterd/examples/plan11-daemon-retention.rs
docs/plans/0011-burst-output-memory-retention.md
tools/benchmark/retention-v2-schema.json
tools/benchmark/run-graphical-retention-v2.py
tools/performance/run-publication-metrics-overhead.py
bash: sha256sum -c SHA256SUMS
bash: printf 'git diff: '; git diff | sha256sum; printf 'git di...
bash: set -euo pipefail
printf '%-80s %-64s %-64s\n' FILE CURRE...
bash: python - <<'PY'
import json,pathlib,statistics
p=pathlib....
bash: python - <<'PY'
import json,hashlib,pathlib
p=pathlib.Pat...
bash: printf '%s\n' '--- validation tail ---'; tail -80 docs/be...
sha256sum: SHA256SUMS: No such file or directory
Command exited with code 1
git diff: ef4db184f8c64cfde545386cc1409a6e47b519c6221099fafac9a3fb7bce5540  -
git diff --binary: ef4db184f8c64cfde545386cc1409a6e47b519c6221099fafac9a3fb7bce5540  -
stored patch: sha256sum: provenance/candidate-tracked.patch: No such file or directory
patch headers current:
AGENTS.md
crates/splinterd/examples/phase9-daemon-benchmark.rs
crates/splinterd/src/lib.rs
crates/splinterd/src/live.rs
crates/splinterd/src/main.rs
crates/splinterm/src/wayland.rs
tools/benchmark/metrics.py
tools/benchmark/run-graphical-idle.py
tools/benchmark/run-graphical-output.py
tools/benchmark/test_benchmark.py
patch headers artifact:
grep: provenance/candidate-tracked.patch: No such file or directory
Command exited with code 2
FILE                                                                             CURRENT                                                          TAR                                                             
crates/splinterd/examples/plan11-daemon-retention.rs                             1d62469e3e61ee3648812860800203f06b9643306e9fce7bc197239e55eae674 1d62469e3e61ee3648812860800203f06b9643306e9fce7bc197239e55eae674
docs/plans/0011-burst-output-memory-retention.md                                 09195a0b7c9c053df000bbe9c8fff5467c657201bdc32b7ff2b8af3949e2e51a 09195a0b7c9c053df000bbe9c8fff5467c657201bdc32b7ff2b8af3949e2e51a
tools/benchmark/retention-v2-schema.json                                         81ffd2ab01b3f9c08e1bdd0062ee557d8bdb7f7b8b1564a436804844ca1bba5b 81ffd2ab01b3f9c08e1bdd0062ee557d8bdb7f7b8b1564a436804844ca1bba5b
tools/benchmark/run-graphical-retention-v2.py                                    eb86a9592911db10233eeeea73d46fcc25b32ff64b15f43f86db4005c2b85cf3 eb86a9592911db10233eeeea73d46fcc25b32ff64b15f43f86db4005c2b85cf3
tools/performance/run-publication-metrics-overhead.py                            1004055450bddebda61ec5e12c2f4ea2322eb0be2d7b5d50963b56a296ea979e 1004055450bddebda61ec5e12c2f4ea2322eb0be2d7b5d50963b56a296ea979e
TAR MEMBERS
crates/splinterd/examples/plan11-daemon-retention.rs
docs/plans/0011-burst-output-memory-retention.md
tools/benchmark/retention-v2-schema.json
tools/benchmark/run-graphical-retention-v2.py
tools/performance/run-publication-metrics-overhead.py
warmup control 2 valid 2 cleanup 2 ws8 2 DP-2 2 no-focus 2
warmup candidate 2 valid 2 cleanup 2 ws8 2 DP-2 2 no-focus 2
measured control 10 valid 10 cleanup 10 ws8 10 DP-2 10 no-focus 10
  retained_growth_bytes 73871360.0
  cpu_ticks 19.0
  trigger_to_visible_marker_ns 396125124.0
measured candidate 10 valid 10 cleanup 10 ws8 10 DP-2 10 no-focus 10
  retained_growth_bytes 81838080.0
  cpu_ticks 76.5
  trigger_to_visible_marker_ns 615855954.5
candidate_splinterd True 5125496 e66e939de467f7143c3695b231734f0ed86f2a40804bd86f00ec7dbf58586b79
candidate_splinterm True 7662296 55879f8085797fc22ec3b4ef9911ec0b8fef5a94d92da8a701410dae422bb1a7
candidate_splinterm-pty-child True 490200 90df4a966566f6c084328090aa6d1d795f78aa7d3cc483e0d6ef6dd00eec9f77
control_splinterd True 5095696 7ff18e4adcb6e80316b1482e683c72322c2b2d630ade27d95c40c4bddcd37ca0
control_splinterm True 7662296 55879f8085797fc22ec3b4ef9911ec0b8fef5a94d92da8a701410dae422bb1a7
control_splinterm-pty-child True 490184 9d3225b2dc0af4d8ca2132a2caa1fdfe9d5fce8ef61351ef40c5abde6c96f6c7
CANDIDATE SHA LIST
55879f8085797fc22ec3b4ef9911ec0b8fef5a94d92da8a701410dae422bb1a7  /tmp/splinterm-plan0011-final-candidate-bin/splinterm
e66e939de467f7143c3695b231734f0ed86f2a40804bd86f00ec7dbf58586b79  /tmp/splinterm-plan0011-final-candidate-bin/splinterd
90df4a966566f6c084328090aa6d1d795f78aa7d3cc483e0d6ef6dd00eec9f77  /tmp/splinterm-plan0011-final-candidate-bin/splinterm-pty-child
d5a83551e5d75b62432691b552a1200a931c7347a00e846ce42efce800ec6769  /tmp/splinterm-plan0011-final-candidate-bin/plan11-daemon-retention
--- validation tail ---
test scrollback_pages_walk_older_rows_without_overlap ... ok
test snapshot_resolves_composed_content_and_semantic_attributes ... ok
test sparse_wrapped_reflow_assigns_ids_in_chronological_order ... ok
test stable_history_ids_survive_ring_movement_and_generation_changes_reset_them ... ok
test updates_report_rows_cursor_scroll_and_full_snapshot_damage ... ok
test result: ok. 15 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
   Doc-tests splinterd
running 0 tests
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
   Doc-tests splinterm
running 0 tests
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
   Doc-tests splinterm_automation_client
running 0 tests
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
   Doc-tests splinterm_core
running 0 tests
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
   Doc-tests splinterm_filemap
running 0 tests
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
   Doc-tests splinterm_freetype
running 0 tests
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
   Doc-tests splinterm_mcp
running 0 tests
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
   Doc-tests splinterm_pixman
running 0 tests
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
   Doc-tests splinterm_protocol
running 0 tests
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
   Doc-tests splinterm_pty
running 0 tests
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
   Doc-tests splinterm_relay
running 0 tests
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
   Doc-tests splinterm_terminal
running 0 tests
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
--- failures/errors ---
bash: cd docs/benchmarks/artifacts/2026-07-27-plan0011-scroll-b...
bash: A=docs/benchmarks/artifacts/2026-07-27-plan0011-scroll-bo...
bash: A=docs/benchmarks/artifacts/2026-07-27-plan0011-scroll-bo...
bash: python - <<'PY'
import json,pathlib
p=pathlib.Path('docs/...
candidate-binaries.sha256: OK
graphical/control-candidate/__pycache__/run.cpython-314.pyc: OK
graphical/control-candidate/process-attribution.json: OK
graphical/control-candidate/progress.json: OK
graphical/control-candidate/raw/measured/00/candidate/runner.stderr: OK
graphical/control-candidate/raw/measured/00/candidate/runner.stdout: OK
graphical/control-candidate/raw/measured/00/candidate/splinterm-retention.json: OK
graphical/control-candidate/raw/measured/00/control/runner.stderr: OK
graphical/control-candidate/raw/measured/00/control/runner.stdout: OK
graphical/control-candidate/raw/measured/00/control/splinterm-retention.json: OK
graphical/control-candidate/raw/measured/01/candidate/runner.stderr: OK
graphical/control-candidate/raw/measured/01/candidate/runner.stdout: OK
graphical/control-candidate/raw/measured/01/candidate/splinterm-retention.json: OK
graphical/control-candidate/raw/measured/01/control/runner.stderr: OK
graphical/control-candidate/raw/measured/01/control/runner.stdout: OK
graphical/control-candidate/raw/measured/01/control/splinterm-retention.json: OK
graphical/control-candidate/raw/measured/02/candidate/runner.stderr: OK
graphical/control-candidate/raw/measured/02/candidate/runner.stdout: OK
graphical/control-candidate/raw/measured/02/candidate/splinterm-retention.json: OK
graphical/control-candidate/raw/measured/02/control/runner.stderr: OK
graphical/control-candidate/raw/measured/02/control/runner.stdout: OK
graphical/control-candidate/raw/measured/02/control/splinterm-retention.json: OK
graphical/control-candidate/raw/measured/03/candidate/runner.stderr: OK
graphical/control-candidate/raw/measured/03/candidate/runner.stdout: OK
graphical/control-candidate/raw/measured/03/candidate/splinterm-retention.json: OK
graphical/control-candidate/raw/measured/03/control/runner.stderr: OK
graphical/control-candidate/raw/measured/03/control/runner.stdout: OK
graphical/control-candidate/raw/measured/03/control/splinterm-retention.json: OK
graphical/control-candidate/raw/measured/04/candidate/runner.stderr: OK
graphical/control-candidate/raw/measured/04/candidate/runner.stdout: OK
graphical/control-candidate/raw/measured/04/candidate/splinterm-retention.json: OK
graphical/control-candidate/raw/measured/04/control/runner.stderr: OK
graphical/control-candidate/raw/measured/04/control/runner.stdout: OK
graphical/control-candidate/raw/measured/04/control/splinterm-retention.json: OK
graphical/control-candidate/raw/measured/05/candidate/runner.stderr: OK
graphical/control-candidate/raw/measured/05/candidate/runner.stdout: OK
graphical/control-candidate/raw/measured/05/candidate/splinterm-retention.json: OK
graphical/control-candidate/raw/measured/05/control/runner.stderr: OK
graphical/control-candidate/raw/measured/05/control/runner.stdout: OK
graphical/control-candidate/raw/measured/05/control/splinterm-retention.json: OK
graphical/control-candidate/raw/measured/06/candidate/runner.stderr: OK
graphical/control-candidate/raw/measured/06/candidate/runner.stdout: OK
graphical/control-candidate/raw/measured/06/candidate/splinterm-retention.json: OK
graphical/control-candidate/raw/measured/06/control/runner.stderr: OK
graphical/control-candidate/raw/measured/06/control/runner.stdout: OK
graphical/control-candidate/raw/measured/06/control/splinterm-retention.json: OK
graphical/control-candidate/raw/measured/07/candidate/runner.stderr: OK
graphical/control-candidate/raw/measured/07/candidate/runner.stdout: OK
graphical/control-candidate/raw/measured/07/candidate/splinterm-retention.json: OK
graphical/control-candidate/raw/measured/07/control/runner.stderr: OK
graphical/control-candidate/raw/measured/07/control/runner.stdout: OK
graphical/control-candidate/raw/measured/07/control/splinterm-retention.json: OK
graphical/control-candidate/raw/measured/08/candidate/runner.stderr: OK
graphical/control-candidate/raw/measured/08/candidate/runner.stdout: OK
graphical/control-candidate/raw/measured/08/candidate/splinterm-retention.json: OK
graphical/control-candidate/raw/measured/08/control/runner.stderr: OK
graphical/control-candidate/raw/measured/08/control/runner.stdout: OK
graphical/control-candidate/raw/measured/08/control/splinterm-retention.json: OK
graphical/control-candidate/raw/measured/09/candidate/runner.stderr: OK
graphical/control-candidate/raw/measured/09/candidate/runner.stdout: OK
graphical/control-candidate/raw/measured/09/candidate/splinterm-retention.json: OK
graphical/control-candidate/raw/measured/09/control/runner.stderr: OK
graphical/control-candidate/raw/measured/09/control/runner.stdout: OK
graphical/control-candidate/raw/measured/09/control/splinterm-retention.json: OK
graphical/control-candidate/raw/warmup/00/candidate/runner.stderr: OK
graphical/control-candidate/raw/warmup/00/candidate/runner.stdout: OK
graphical/control-candidate/raw/warmup/00/candidate/splinterm-retention.json: OK
graphical/control-candidate/raw/warmup/00/control/runner.stderr: OK
graphical/control-candidate/raw/warmup/00/control/runner.stdout: OK
graphical/control-candidate/raw/warmup/00/control/splinterm-retention.json: OK
graphical/control-candidate/raw/warmup/01/candidate/runner.stderr: OK
graphical/control-candidate/raw/warmup/01/candidate/runner.stdout: OK
graphical/control-candidate/raw/warmup/01/candidate/splinterm-retention.json: OK
graphical/control-candidate/raw/warmup/01/control/runner.stderr: OK
graphical/control-candidate/raw/warmup/01/control/runner.stdout: OK
graphical/control-candidate/raw/warmup/01/control/splinterm-retention.json: OK
graphical/control-candidate/run.log: OK
graphical/control-candidate/run.py: OK
graphical/control-candidate/summary.json: OK
graphical/smoke/preflight.json: OK
graphical/smoke/splinterm-retention.json: OK
provenance/candidate-tracked.patch: OK
provenance/git-status.txt: OK
provenance/identities.json: OK
provenance/toolchain.txt: OK
provenance/untracked-source-files.txt: OK
provenance/untracked-source.tar: OK
summary.json: OK
summary.md: OK
validation/cargo-test-workspace-serial.log: OK
current diff ef4db184f8c64cfde545386cc1409a6e47b519c6221099fafac9a3fb7bce5540  -
artifact patch ef4db184f8c64cfde545386cc1409a6e47b519c6221099fafac9a3fb7bce5540  docs/benchmarks/artifacts/2026-07-27-plan0011-scroll-bound-fix-3/provenance/candidate-tracked.patch
artifact stated ef4db184f8c64cfde545386cc1409a6e47b519c6221099fafac9a3fb7bce5540
cmp exact: yes
--- line citations plan ---
     1	# Plan 0011: burst-output memory-retention optimization
     2	
     3	- **Status:** Final closure no-go — daemon retention improved, but randomized graphical aggregate/client retention and responsiveness regressed
     4	- **Release decision:** Do not tag `beta1` until this pass has recorded validation and review
     5	- **Parent plan:** [Plan 0010](0010-full-performance-optimization-pass.md), especially Slice 5
     6	- **Behavioral authority:** Foot 1.27.0 commit `3c5b584b0eafa772eb4376fb6eaf6643399e190e`
     7	- **Primary evidence:** [five-terminal retention matrix](../benchmarks/artifacts/2026-07-23-five-terminal-retention/summary.md) and [first measured performance pass](../spikes/artifacts/0026-performance-optimization-pass/README.md)
     8	
     9	## Decision
    10	
    11	Run one focused, measurement-led pass against burst-output memory retention. CPU,
    12	responsiveness, correctness, and idle behavior are release-candidate quality and
   245	  retained snapshots after teardown. Delayed retained growth fell from 43.88
   246	  MiB to 4.81 MiB. Producer-batch completion is event-driven through Tokio
   247	  `Notify`; the focused regression records one park/wake per synchronous PTY
   248	  read rather than polling. The final full serial workspace passed; the earlier
   249	  ordinary concurrent daemon suite reproduced only the documented policy
   250	  timeout, whose isolated run passed in 14.82 seconds. Successful fast 1,000-row
   251	  materialization still retained 34.33 MiB, naming required-history
   252	  materialization as Slice 3's next measured class. Fresh independent review
   253	  accepted the bounded mailbox, revision/exit/resnapshot semantics, exact
   254	  ownership accounting, and event-driven producer completion with no blockers.
   255	
   256	- **Slice 3 accepted (2026-07-27):** [the dated Slice 3 artifact](../benchmarks/artifacts/2026-07-27-plan0011-slice3/summary.md)
   257	  records a private fail-safe compact history policy. Exact contiguous updates
   258	  own the complete visible grid plus either no history rows or only a proven
   259	  normal-screen full-height forward-scroll append tail. Full snapshot,
   260	  dimensions/reflow, clear/replacement, alternate-screen, generation change,
   261	  reverse/partial scroll, and unexplained scrollback remain full-history
   262	  fallbacks. Attach, explicit snapshots, paging, search, protocol DTOs, renderer
   263	  state, and all configured limits are unchanged. Append-delta history is wire
   264	  byte-identical to full materialization after the daemon selects the exact
   265	  appended tail, and private policy/revision mismatch requires resnapshot. Fast
   266	  retained growth fell 44.50% from 34.33 MiB to 19.05 MiB, with zero overflow
   267	  and one retained snapshot; delayed growth was 5.46 MiB with deterministic
   268	  saturation, and two fast subscribers retained 20.41 MiB with aggregate
   269	  snapshot high water two. Direct package checks and the bounded serial
   270	  workspace retry passed. The ordinary concurrent daemon run had the known
   271	  policy timeout plus one phase-8 timing failure, both passing exact isolated;
   272	  the first serial workspace attempt had one unrelated MCP controller flake,
   273	  whose exact isolated test and bounded retry passed. Fresh independent review
   274	  accepted the selective history policy, fail-safe fallbacks, exact mailbox
   275	  policy/revision pairing, append/trim metadata, row identities, wire identity,
   276	  and unchanged public/protocol/limit boundaries with no blockers. The 19.05 MiB
   277	  result satisfies the preferred bounded whole-plan target (at most 20 MiB and
   278	  at least 40% below control), but not the 17 MiB stretch target. Graphical and
   279	  comparative closure evidence remains open.
   280	
   281	- **Final graphical no-go (2026-07-27):** [the corrected final artifact](../benchmarks/artifacts/2026-07-27-plan0011-scroll-bound-fix-3/summary.md)
   282	  records the exact rebuilt candidate, final serial workspace pass, guarded smoke,
   283	  and ten-sample-per-variant randomized clean-HEAD comparison. The candidate
   284	  daemon improved from 34.11 MiB to 21.17 MiB median retained RSS (37.9%), but
   285	  the client regressed from 36.31 MiB to 56.84 MiB (56.5%); aggregate retained
   286	  growth regressed from 70.45 MiB to 78.05 MiB (10.78%), with worse CPU and
   287	  marker latency. The optimization therefore shifts high-water allocation from
   288	  daemon snapshot ownership into a large coalesced client update. The required
   289	  40% aggregate improvement is not established, so Foot/Kitty/Ghostty were not
   290	  run and `beta1` remains forbidden. Correctness fallbacks keep oversized
   335	  passed all unit and binary tests and 15 of 16 integrations; the documented
   336	  `parent_policy_snapshot_excludes_new_splint_until_reload` suite-wide timeout
   337	  reproduced, while its exact isolated run passed in 14.69 seconds. Fresh
   338	  independent review, instrumentation-overhead comparison, current-source
   339	  repeated-cycle RSS/PSS evidence, and all graphical gates remain open; no
   340	  slice closure is claimed.
   341	
   342	## Review and evidence
   343	
   344	Keep one active-worktree writer. Use one measured hypothesis per implementation
   345	commit or clearly separated diff slice. Record rejected experiments. Require a
   346	fresh read-only review after daemon publication ownership changes and before
   347	closure. Do not claim a slice complete without both recorded validation evidence
   348	and recorded review.
   349	
   350	## Stop-loss
   351	
   352	Stop and reassess when:
   353	
   354	- a candidate cannot name the measured retained class it reduces;
   355	- two controlled experiments fail to improve that class;
   356	- memory falls only by moving bytes between daemon, client, SHM, or an unmeasured
   357	  mapping;
   358	- resnapshot, full-reload, redraw, or wakeup frequency rises;
   359	- output CPU, child-write responsiveness, input, resize, or idle regresses beyond
   360	  the accepted control allowance;
   361	- correctness requires weakening revision, history, or reconstruction semantics;
   362	- limits must be reduced to show improvement; or
   363	- beating Ghostty requires a major renderer/protocol rewrite.
   364	
   365	## Completion record
--- line citations summary ---
     1	# Plan 0011 final no-go
     2	
     3	**Decision: do not close Plan 0011 as release-ready and do not tag `beta1`.**
     4	
     5	## Correctness
     6	
     7	Oversized coalesced scroll batches now fall back to bounded final-state viewport patches. Oversized append history uses the existing bounded `HistoryTransition::Replace`; no protocol limits, DTOs, or wire types were widened. Focused daemon/client/protocol tests and the full serial workspace suite pass.
     8	
     9	## Non-graphical evidence
    10	
    11	The corrected five-cycle/120-second daemon workload retained 13.68 MiB RSS and 9.42 MiB private-anonymous memory with zero overflow. Slice 4 allocator diagnostics still justify no allocator-specific product reclamation or manual trim.
    12	
    13	## Graphical evidence
    14	
    15	The final smoke passed workspace 8 / DP-2 placement, no-focus, marker, identity, and cleanup guards. The randomized clean-HEAD comparison completed with two warmups and ten measured samples per variant.
    16	
    17	| Metric | Control median | Candidate median | Decision |
    18	|---|---:|---:|---|
    19	| Aggregate retained RSS | 70.45 MiB | 78.05 MiB | candidate 10.78% worse |
    20	| Marker latency | 396.13 ms | 615.86 ms | regression |
    21	| CPU ticks | 19.0 | 76.5 | regression |
    22	| Daemon retained RSS | 34.11 MiB | 21.17 MiB | improvement |
    23	| Client retained RSS | 36.31 MiB | 56.84 MiB | regression |
    24	
    25	The daemon optimization works, but it moves high-water pressure into the client through a large coalesced update. The required 40% aggregate improvement is not established. Foot/Kitty/Ghostty comparisons were therefore correctly skipped.
    26	
    27	## Next architecture
    28	
    29	A future plan may evaluate bounded intermediate compact checkpoints/publication batches so fast clients receive protocol-sized updates while delayed subscribers still retain at most one compact snapshot. That is an architectural continuation, not a closure or Slice 4 reclamation tweak.
    30	
    31	Exact source provenance, candidate/control binary hashes, raw records, process attribution, and serial validation are retained here.
--- line citations summary json ---
     1	{
     2	  "blocker": "candidate aggregate retained growth, client retained growth, CPU, and marker latency regress against clean-HEAD control",
     3	  "candidate_reduction_percent": -10.78458552813973,
     4	  "closure_complete": false,
     5	  "comparative_terminals_run": false,
     6	  "decision": "no-go",
     7	  "graphical_matrix_valid": true,
     8	  "graphical_smoke_valid": true,
     9	  "improvement_established": false,
    10	  "next_architecture": "bounded intermediate compact checkpoints/publication batches that preserve protocol-sized client updates without restoring delayed-subscriber snapshot retention",
    11	  "process_attribution": {
    12	    "candidate": {
    13	      "python": {
    14	        "max_mib": 0.0,
    15	        "median_mib": 0.0,
    16	        "min_mib": 0.0,
    17	        "samples": 10
    18	      },
    19	      "splinterd": {
    20	        "max_mib": 21.45703125,
    21	        "median_mib": 21.171875,
    22	        "min_mib": 20.40625,
    23	        "samples": 10
    24	      },
    25	      "splinterm": {
    26	        "max_mib": 59.5859375,
    27	        "median_mib": 56.8359375,
    28	        "min_mib": 52.41796875,
    29	        "samples": 10
    30	      }
    31	    },
    32	    "control": {
    33	      "python": {
    34	        "max_mib": 0.0,
    35	        "median_mib": 0.0,
    36	        "min_mib": 0.0,
    37	        "samples": 10
    38	      },
    39	      "splinterd": {
    40	        "max_mib": 34.30078125,
    41	        "median_mib": 34.109375,
    42	        "min_mib": 33.9140625,
    43	        "samples": 10
    44	      },
    45	      "splinterm": {
    46	        "max_mib": 36.4296875,
    47	        "median_mib": 36.310546875,
    48	        "min_mib": 36.11328125,
    49	        "samples": 10
    50	      }
    51	    }
    52	  },
    53	  "samples_per_variant": 10,
    54	  "schema": "splinterm.plan0011.final-no-go.v1",
    55	  "slice4_product_change_justified": false,
    56	  "slices_1_to_3_implementation_valid": true,
    57	  "tag_allowed": false
    58	}
--- code citations ---
  5510	    current: &[splinterd::LiveRow],
  5511	    index: usize,
  5512	) -> bool {
  5513	    previous.get(index) != current.get(index)
  5514	}
  5515	
  5516	fn wire_scrollback_update(
  5517	    rows: &[splinterd::LiveRow],
  5518	    scrollback: splinterm_terminal::ScrollbackSnapshot,
  5519	    previous_history: HistoryState,
  5520	    reflow: bool,
  5521	    appended_rows: usize,
  5522	) -> TerminalScrollbackUpdate {
  5523	    let transition = if scrollback.history_generation != previous_history.generation {
  5524	        if reflow {
  5525	            HistoryTransition::Reflow
  5526	        } else if scrollback.available_rows == 0 {
  5527	            HistoryTransition::Clear
  5528	        } else {
  5529	            HistoryTransition::Replace
  5530	        }
  5531	    } else if appended_rows > 0 && appended_rows <= usize::from(MAX_ROWS) {
  5532	        HistoryTransition::Append {
  5533	            appended_rows,
  5534	            trimmed_rows: previous_history
  5535	                .available_rows
  5536	                .saturating_add(appended_rows)
  5537	                .saturating_sub(scrollback.available_rows),
  5538	        }
  5539	    } else {
  5540	        HistoryTransition::Replace
  5541	    };
  5542	    let maximum_rows = match transition {
  5543	        HistoryTransition::Append { appended_rows, .. } => {
  5544	            appended_rows.min(MAX_SNAPSHOT_SCROLLBACK_ROWS)
  5545	        }
  5546	        HistoryTransition::Clear | HistoryTransition::Reflow | HistoryTransition::Replace => {
  5547	            MAX_SNAPSHOT_SCROLLBACK_ROWS
  5548	        }
  5549	    };
  5550	    let first = rows.len().saturating_sub(maximum_rows);
  5551	    let rows: Vec<_> = rows[first..].iter().cloned().map(wire_row).collect();
  5552	    TerminalScrollbackUpdate {
  5553	        transition,
  5554	        history_generation: scrollback.history_generation,
  5555	        oldest_available_row_id: scrollback.oldest_available_row_id,
  5556	        newest_available_row_id: scrollback.newest_available_row_id,
  5557	        omitted_oldest_rows: scrollback.available_rows.saturating_sub(rows.len()),
  5558	        available_rows: scrollback.available_rows,
  5559	        rows,
  5560	    }
  5561	}
  5562	
  5563	fn bound_wire_scrolls(scrolls: &mut Vec<TerminalScroll>, damaged: &mut [bool]) {
  5564	    if scrolls.len() > MAX_UPDATE_SCROLLS {
  5565	        // Scroll operations are an optimization over the authoritative final rows. A
  5566	        // coalesced burst can contain more scroll damage records than one wire update
  5567	        // permits, so fall back to bounded final-state viewport patches rather than
  5568	        // widening the protocol limit or emitting a semantically incomplete prefix.
  5569	        scrolls.clear();
  5570	        damaged.fill(true);
  5571	    }
  5572	}
  5573	
  5574	fn wire_update(
  5575	    updates: &[TerminalUpdate],
  5576	    snapshot: &LiveSnapshot,
  5577	    previous_revision: u64,
  5578	    previous_history: HistoryState,
  5579	    previous_visible_rows: &[splinterd::LiveRow],
  5580	    include_images: bool,
  5581	) -> Result<WireTerminalUpdate, ProtocolError> {
  5582	    let mut damaged = vec![false; snapshot.visible_rows.len()];
  5583	    let mut scrolls = Vec::new();
  5584	    let mut cursor = false;
  5585	    let mut title = false;
  5586	    let mut modes = false;
  5587	    let mut palette = false;
  5588	    let mut dimensions = false;
  5589	    let mut scrollback = false;
  5590	    let mut images = false;
  5591	    let mut reflow = false;
  5592	    let mut appended_rows = 0_usize;
  5593	    for damage in updates.iter().flat_map(TerminalUpdate::damage) {
  5594	        match damage {
  5595	            TerminalDamage::FullSnapshot => {
  5596	                damaged.fill(true);
  5597	                cursor = true;
  5598	                title = true;
  5599	                modes = true;
  5600	                palette = true;
  5601	                dimensions = true;
  5602	                scrollback = true;
  5603	                images = true;
  5604	            }
  5605	            TerminalDamage::Viewport => damaged.fill(true),
  5606	            TerminalDamage::Rows { start, end } => {
  5607	                for item in damaged.iter_mut().take(*end).skip(*start) {
  5608	                    *item = true;
  5609	                }
  5610	            }
  5611	            TerminalDamage::Scroll {
  5612	                direction,
  5613	                region,
  5614	                rows,
  5615	            } => {
  5616	                let start = usize::try_from(region.start()).map_err(|_| internal())?;
  5617	                let end = usize::try_from(region.end()).map_err(|_| internal())?;
  5618	                if *direction == ScrollDirection::Forward
  5619	                    && start == 0
  5620	                    && end == snapshot.dimensions.rows
  5621	                    && snapshot.active_screen == ActiveScreen::Normal
  5622	                {
  5623	                    appended_rows = appended_rows.saturating_add(*rows);
  5624	                }
  5625	                for item in damaged.iter_mut().take(end).skip(start) {
  5626	                    *item = true;
  5627	                }
  5628	                scrolls.push(TerminalScroll {
  5629	                    direction: match direction {
  5630	                        ScrollDirection::Forward => WireScrollDirection::Forward,
  5631	                        ScrollDirection::Reverse => WireScrollDirection::Reverse,
  5632	                    },
  5633	                    start_row: start,
  5634	                    end_row: end,
  5635	                    rows: *rows,
  5636	                });
  5637	            }
  5638	            TerminalDamage::Cursor { .. } => cursor = true,
  5639	            TerminalDamage::Modes => modes = true,
  5640	            TerminalDamage::Dimensions => {
  5641	                dimensions = true;
  5642	                reflow = true;
  5643	                damaged.fill(true);
  5644	            }
  5645	            TerminalDamage::Title => title = true,
  5646	            TerminalDamage::Palette { .. } => palette = true,
  5647	            TerminalDamage::Scrollback => scrollback = true,
  5648	            TerminalDamage::Images { .. } => {
  5649	                damaged.fill(true);
  5650	                images = true;
  5651	            }
  5652	        }
  5653	    }
  5654	    bound_wire_scrolls(&mut scrolls, &mut damaged);
  5655	    let wire_scrollback = if scrollback {
  5656	        Some(wire_scrollback_update(
  5657	            &snapshot.scrollback_rows,
  5658	            snapshot.scrollback,
  5659	            previous_history,
  5660	            reflow,
  5661	            appended_rows,
  5662	        ))
  5663	    } else {
  5664	        None
  5665	    };
  5666	    let position = snapshot.cursor.cursor.position();
  5667	    let rows = damaged
  5668	        .into_iter()
  5669	        .enumerate()
  5670	        .filter(|(index, changed)| {
  5671	            *changed && visible_row_changed(previous_visible_rows, &snapshot.visible_rows, *index)
  5672	        })
  5673	        .map(|(index, _)| TerminalRowPatch {
  5674	            index,
  5675	            row: wire_row(snapshot.visible_rows[index].clone()),
  5676	        })
  5677	        .collect();
  5678	    Ok(WireTerminalUpdate {
  5679	        base_revision: previous_revision,
  5680	        revision: updates.last().ok_or_else(internal)?.revision().value(),
  5681	        rows,
  5682	        scrolls,
  5683	        cursor: cursor.then_some(TerminalCursor {
  5684	            column: position.column,
  5685	            row: position.row,
  5686	            deferred_wrap: snapshot.cursor.cursor.deferred_wrap(),
  5687	        }),
  5688	        title: title.then(|| snapshot.title.clone()),
  5689	        input_modes: modes.then_some(wire_modes(snapshot.modes)),
  5690	        active_screen: modes.then_some(wire_active_screen(snapshot.active_screen)),
  5691	        palette: palette.then(|| snapshot.palette.to_vec()),
  5692	        default_colors: palette.then_some(snapshot.default_colors),
  5693	        columns: dimensions.then_some(snapshot.dimensions.columns),
  5694	        row_count: dimensions.then_some(snapshot.dimensions.rows),
  5695	        scrollback: wire_scrollback,
  5696	        images: (include_images && images).then(|| Box::new(wire_image_plane(snapshot))),
  5697	    })
  5698	}
  5950	    use super::*;
  5951	    use std::time::{SystemTime, UNIX_EPOCH};
  5952	
  5953	    #[test]
  5954	    fn public_live_row_materializes_unchanged_wire_content() {
  5955	        let attributes = splinterm_terminal::Attributes::default().into();
  5956	        let row = splinterd::LiveRow {
  5957	            row_id: Some(7),
  5958	            linebreak: true,
  5959	            cells: vec![splinterd::LiveCell {
  5960	                content: "A".to_owned(),
  5961	                spacer_remaining: None,
  5962	                attributes,
  5963	            }],
  5964	        };
  5965	        let expected = TerminalRow {
  5966	            row_id: Some(7),
  5967	            linebreak: true,
  5968	            cells: vec![TerminalCell {
  5969	                content: "A".to_owned(),
  5970	                spacer_remaining: None,
  5971	                attributes: CellAttributes::default(),
  5972	            }],
  5973	        };
  5974	
  5975	        let actual = wire_row(row);
  5976	        assert_eq!(actual, expected);
  5977	        assert_eq!(
  5978	            serde_json::to_vec(&actual).unwrap(),
  5979	            serde_json::to_vec(&expected).unwrap()
  5980	        );
  5981	    }
  5982	
  5983	    #[test]
  5984	    fn oversized_append_batch_falls_back_to_bounded_history_replace() {
  5985	        let attributes = splinterm_terminal::Attributes::default().into();
  5986	        let rows = (1..=20)
  5987	            .map(|row_id| splinterd::LiveRow {
  5988	                row_id: Some(row_id),
  5989	                linebreak: true,
  5990	                cells: vec![splinterd::LiveCell {
  5991	                    content: format!("row-{row_id}"),
  5992	                    spacer_remaining: None,
  5993	                    attributes,
  5994	                }],
  5995	            })
  5996	            .collect::<Vec<_>>();
  5997	        let scrollback = splinterm_terminal::ScrollbackSnapshot {
  5998	            history_generation: 7,
  5999	            oldest_available_row_id: Some(1),
  6000	            newest_available_row_id: Some(20),
  6001	            available_rows: 20,
  6002	            returned_rows: 20,
  6003	            omitted_oldest_rows: 0,
  6004	        };
  6005	        let previous = HistoryState {
  6006	            revision: 10,
  6007	            generation: 7,
  6008	            available_rows: 0,
  6009	        };
  6010	
  6011	        let update = wire_scrollback_update(
  6012	            &rows,
  6013	            scrollback,
  6014	            previous,
  6015	            false,
  6016	            usize::from(MAX_ROWS) + 1,
  6017	        );
  6018	
  6019	        assert_eq!(update.transition, HistoryTransition::Replace);
  6020	        assert_eq!(update.rows.len(), MAX_SNAPSHOT_SCROLLBACK_ROWS);
  6021	        assert_eq!(update.omitted_oldest_rows, 4);
  6022	        let wire = WireTerminalUpdate {
  6023	            base_revision: 10,
  6024	            revision: 20,
  6025	            rows: Vec::new(),
  6026	            scrolls: Vec::new(),
  6027	            cursor: None,
  6028	            title: None,
  6029	            input_modes: None,
  6030	            active_screen: None,
  6031	            palette: None,
  6032	            default_colors: None,
  6033	            columns: None,
  6034	            row_count: None,
  6035	            scrollback: Some(update),
  2010	fn apply_scrollback_update(
  2011	    snapshot: &mut TerminalSnapshot,
  2012	    scrollback: splinterm_protocol::TerminalScrollbackUpdate,
  2013	) -> Result<()> {
  2014	    match scrollback.transition {
  2015	        HistoryTransition::Append { .. }
  2016	            if scrollback.history_generation != snapshot.history_generation =>
  2017	        {
  2018	            anyhow::bail!("history append changed generation");
  2019	        }
  2020	        HistoryTransition::Clear | HistoryTransition::Reflow
  2021	            if scrollback.history_generation <= snapshot.history_generation =>
  2022	        {
  2023	            anyhow::bail!("history reset did not change generation");
  2024	        }
  2025	        _ => {}
  2026	    }
  2027	    let preserve_cached = scrollback.history_generation == snapshot.history_generation
  2028	        && matches!(
  2029	            scrollback.transition,
  2030	            HistoryTransition::Append { .. } | HistoryTransition::Replace
  2031	        );
  2032	    let first_returned = scrollback.rows.first().and_then(|row| row.row_id);
  2033	    let oldest_available = scrollback.oldest_available_row_id;
  2034	    let mut rows = if preserve_cached {
  2035	        snapshot
  2036	            .scrollback_rows
  2037	            .iter()
  2038	            .filter(|row| {
  2039	                row.row_id
  2040	                    .zip(oldest_available)
  2041	                    .is_some_and(|(id, oldest)| id >= oldest)
  2042	                    && row
  2043	                        .row_id
  2044	                        .zip(first_returned)
  2045	                        .is_some_and(|(id, first)| id < first)
  2046	            })
  2047	            .cloned()
  2048	            .collect::<Vec<_>>()
  2049	    } else {
  2050	        Vec::new()
  2051	    };
  2052	    rows.extend(scrollback.rows);
  2053	    bound_history_cache(&mut rows, false);
  2054	    snapshot.history_generation = scrollback.history_generation;
  2055	    snapshot.oldest_available_scrollback_row_id = scrollback.oldest_available_row_id;
  2056	    snapshot.newest_available_scrollback_row_id = scrollback.newest_available_row_id;
  2057	    snapshot.scrollback_rows = rows;
  2058	    snapshot.available_scrollback_rows = scrollback.available_rows;
  2059	    snapshot.omitted_oldest_scrollback_rows = snapshot
  2060	        .available_scrollback_rows
  2061	        .saturating_sub(snapshot.scrollback_rows.len());
  2062	    Ok(())
  2063	}
  7670	            vec![3, 4, 5, 6]
  7671	        );
  7672	        assert_eq!(current.omitted_oldest_scrollback_rows, 0);
  7673	    }
  7674	
  7675	    #[test]
  7676	    fn bounded_history_replace_applies_visible_marker_without_resync() {
  7677	        let mut current = snapshot(SplintId::new(), 7, 10);
  7678	        current.columns = 1;
  7679	        current.rows = 1;
  7680	        current.visible_rows = vec![blank_row(1)];
  7681	        current.scrollback_rows = (1..=4).map(|id| history_row(id, 0)).collect();
  7682	        current.available_scrollback_rows = 4;
  7683	        current.oldest_available_scrollback_row_id = Some(1);
  7684	        current.newest_available_scrollback_row_id = Some(4);
  7685	        let marker = TerminalRow {
  7686	            row_id: Some(21),
  7687	            linebreak: true,
  7688	            cells: vec![TerminalCell {
  7689	                content: "SPLINTERBENCH_DONE".into(),
  7690	                spacer_remaining: None,
  7691	                attributes: current.visible_rows[0].cells[0].attributes,
  7692	            }],
  7693	        };
  7694	
  7695	        apply_terminal_update(
  7696	            &mut current,
  7697	            TerminalUpdate {
  7698	                base_revision: 10,
  7699	                revision: 11,
  7700	                rows: vec![splinterm_protocol::TerminalRowPatch {
  7701	                    index: 0,
  7702	                    row: marker.clone(),
  7703	                }],
  7704	                scrolls: Vec::new(),
  7705	                cursor: None,
  7706	                title: None,
  7707	                input_modes: None,
  7708	                active_screen: None,
  7709	                palette: None,
  7710	                default_colors: None,
  7711	                columns: None,
  7712	                row_count: None,
  7713	                scrollback: Some(splinterm_protocol::TerminalScrollbackUpdate {
  7714	                    transition: HistoryTransition::Replace,
  7715	                    history_generation: 1,
  7716	                    oldest_available_row_id: Some(1),
  7717	                    newest_available_row_id: Some(20),
  7718	                    rows: vec![history_row(19, 0), history_row(20, 0)],
  7719	                    available_rows: 20,
  7720	                    omitted_oldest_rows: 18,
  7721	                }),
  7722	                images: None,
  7723	            },
  7724	        )
  7725	        .expect("bounded replace update");
  7726	
  7727	        assert_eq!(current.revision, 11);
  7728	        assert_eq!(current.visible_rows[0], marker);
  7729	        assert_eq!(current.newest_available_scrollback_row_id, Some(20));
  7730	    }
  7731	
  7732	    #[test]
  7733	    fn semantic_update_applies_exact_row_cursor_and_title_revision() {
  7734	        let mut current = snapshot(SplintId::new(), 7, 10);
  7735	        current.columns = 2;
  7736	        current.rows = 1;
  7737	        current.visible_rows = vec![blank_row(2)];
  7738	        let row = TerminalRow {
  7739	            row_id: Some(8),
  7740	            linebreak: true,
  7741	            cells: vec![TerminalCell {
  7742	                content: "x".into(),
  7743	                spacer_remaining: None,
  7744	                attributes: current.visible_rows[0].cells[0].attributes,
  7745	            }],
all raw guard deviations: none
Status snapshot vs provenance status diff:
Tool budget soft limit reached after 30 tool calls (soft 30, hard 45). Stop starting new browsing/search work and finalize from the context you already have.
## Review
- **Correct:** The scroll fallback clears an oversized scroll list and marks the bounded viewport rows for final-state patches, retaining existing `MAX_UPDATE_SCROLLS` and row-patch limits (`crates/splinterd/src/main.rs:5563-5571`, `5654-5677`).
- **Correct:** Oversized append history falls back to existing `HistoryTransition::Replace` and caps rows at `MAX_SNAPSHOT_SCROLLBACK_ROWS` (`crates/splinterd/src/main.rs:5516-5560`). No protocol crate, DTO, or limit definition is modified.
- **Correct:** Daemon tests exercise bounded replacement and protocol validation (`crates/splinterd/src/main.rs:5983-6043`) and oversized scroll fallback. Client application is tested through the actual update path, including visible-marker application without resync (`crates/splinterm/src/wayland.rs:2010-2062`, `7675-7729`).
- **Correct:** The smoke record is valid and records workspace 8, DP-2, no initial focus, and verified cleanup. All two warmups and ten measured records per variant independently had those same guards, `valid: true`, and no notes.
- **Correct:** Recalculation from raw records confirms:
  - Aggregate retained growth: 70.45 MiB control versus 78.05 MiB candidate, **10.78% worse**.
  - Daemon: 34.11 MiB versus 21.17 MiB, approximately **37.9% better**.
  - Client: 36.31 MiB versus 56.84 MiB, approximately **56.5% worse**.
  - CPU: 19.0 versus 76.5 ticks.
  - Marker latency: 396.13 ms versus 615.86 ms.
- **Correct:** Because the required 40% aggregate improvement failed, skipping Foot/Kitty/Ghostty follows the declared gate (`docs/plans/0011-burst-output-memory-retention.md:281-290`; artifact `summary.md:17-25`).
- **Correct:** Slice 4 remains a justified no-change conclusion: measured daemon retention is bounded, allocator diagnostics did not establish a beneficial allocator-specific intervention, and no allocator-specific product change appears in the tracked diff (`summary.md:9-11`, `27-29`).
- **Correct:** The plan and summary explicitly record a no-go rather than closure (`docs/plans/0011-burst-output-memory-retention.md:3-4`; `summary.md:1-3`; `summary.json:2-9,53-57`).
- **Correct:** Provenance checks passed:
  - `candidate-tracked.patch` is byte-for-byte identical to the current tracked `git diff`.
  - Every listed untracked source matches its tar member.
  - All present candidate/control binaries match recorded sizes and hashes.
  - `sha256sum -c SHA256SUMS` passes from the artifact directory.
  - Provenance status matches current status, and the index has no staged files.
- **Correct:** The retained serial workspace log ends with passing tests and contains no failed test result, panic, or test-failure marker.
- **Blocker:** No acceptance blocker in the record itself. The documented product blocker—aggregate/client retention, CPU, and latency regression—is genuine and correctly causes the release no-go.
- **Fixed:** None; review was read-only.
- **Note:** The remaining risk is architectural, not an artifact defect: bounded compact checkpoints/publication batches are needed before reconsidering release readiness.
## Explicit decisions
- **Slices 1–3 implementation state:** **ACCEPT**
- **Correctness fallbacks:** **ACCEPT**
- **Slice 4 no-change conclusion:** **ACCEPT**
- **Final no-go evidence:** **ACCEPT**
- **Release decision:** **NO-GO**
- **`beta1` forbidden:** **YES**
```acceptance-report
{
  "criteriaSatisfied": [
    {
      "id": "criterion-1",
      "status": "satisfied",
      "evidence": "No acceptance defects found; exact code and evidence citations are provided. The measured performance regression is a confirmed product blocker and is accurately represented as a release no-go."
    }
  ],
  "changedFiles": [
    "AGENTS.md",
    "crates/splinterd/examples/phase9-daemon-benchmark.rs",
    "crates/splinterd/examples/plan11-daemon-retention.rs",
    "crates/splinterd/src/lib.rs",
    "crates/splinterd/src/live.rs",
    "crates/splinterd/src/main.rs",
    "crates/splinterm/src/wayland.rs",
    "docs/plans/0011-burst-output-memory-retention.md",
    "docs/benchmarks/artifacts/2026-07-26-plan0011-closure/",
    "docs/benchmarks/artifacts/2026-07-27-plan0011-final/",
    "docs/benchmarks/artifacts/2026-07-27-plan0011-guarded-retry/",
    "docs/benchmarks/artifacts/2026-07-27-plan0011-guarded-retry-2/",
    "docs/benchmarks/artifacts/2026-07-27-plan0011-guarded-retry-3/",
    "docs/benchmarks/artifacts/2026-07-27-plan0011-guarded-retry-4/",
    "docs/benchmarks/artifacts/2026-07-27-plan0011-scroll-bound-fix/",
    "docs/benchmarks/artifacts/2026-07-27-plan0011-scroll-bound-fix-2/",
    "docs/benchmarks/artifacts/2026-07-27-plan0011-scroll-bound-fix-3/",
    "docs/benchmarks/artifacts/2026-07-27-plan0011-slice2/",
    "docs/benchmarks/artifacts/2026-07-27-plan0011-slice3/",
    "tools/benchmark/metrics.py",
    "tools/benchmark/retention-v2-schema.json",
    "tools/benchmark/run-graphical-idle.py",
    "tools/benchmark/run-graphical-output.py",
    "tools/benchmark/run-graphical-retention-v2.py",
    "tools/benchmark/test_benchmark.py",
    "tools/performance/run-publication-metrics-overhead.py"
  ],
  "testsAddedOrUpdated": [
    "crates/splinterd/src/live.rs",
    "crates/splinterd/src/main.rs",
    "crates/splinterm/src/wayland.rs",
    "tools/benchmark/test_benchmark.py"
  ],
  "commandsRun": [
    {
      "command": "git status --short; git diff --stat; git diff --cached --name-only",
      "result": "passed",
      "summary": "Inspected accumulated changes and confirmed no staged files."
    },
    {
      "command": "cd docs/benchmarks/artifacts/2026-07-27-plan0011-scroll-bound-fix-3 && sha256sum -c SHA256SUMS",
      "result": "passed",
      "summary": "All final artifact files, including raw records and provenance, passed checksums."
    },
    {
      "command": "cmp -s <(git diff) provenance/candidate-tracked.patch",
      "result": "passed",
      "summary": "Current tracked diff and retained candidate patch are byte-for-byte identical."
    },
    {
      "command": "Compare current listed untracked sources against provenance/untracked-source.tar",
      "result": "passed",
      "summary": "All five listed sources matched their tar members, with no extra tar members."
    },
    {
      "command": "Hash and size-check candidate/control binaries against provenance/identities.json",
      "result": "passed",
      "summary": "All present binaries exactly matched recorded identities."
    },
    {
      "command": "Parse all graphical raw warmup/measured JSON records and recompute medians",
      "result": "passed",
      "summary": "Confirmed two valid warmups and ten valid measured samples per variant, all on workspace 8/DP-2 with no-focus and cleanup guards."
    },
    {
      "command": "Inspect validation/cargo-test-workspace-serial.log for final results and failure markers",
      "result": "passed",
      "summary": "Serial workspace evidence finishes successfully with no failure markers."
    }
  ],
  "validationOutput": [
    "Aggregate candidate retained growth is 10.7846% worse than control.",
    "Daemon median retained growth improves approximately 37.9%; client median regresses approximately 56.5%.",
    "Candidate CPU and marker latency are materially worse.",
    "Final smoke and all randomized records satisfy isolation and cleanup guards.",
    "SHA256SUMS, tracked patch provenance, source bundle provenance, and binary identities all pass.",
    "No staged files."
  ],
  "residualRisks": [
    "The candidate shifts retention and responsiveness cost into the client; release requires a separately planned bounded-checkpoint/publication architecture."
  ],
  "noStagedFiles": true,
  "diffSummary": "Slices 1-3 add compact daemon publication/history handling and bounded wire fallbacks, plus benchmark instrumentation, harnesses, tests, plans, and retained evidence. Final measurements establish a release no-go.",
  "reviewFindings": [
    "no acceptance blockers",
    "release blocker: docs/benchmarks/artifacts/2026-07-27-plan0011-scroll-bound-fix-3/summary.json:2-9 - aggregate/client retention, CPU, and latency regress; no-go is accurate",
    "correct: crates/splinterd/src/main.rs:5516-5571 - bounded Replace and scroll fallback preserve existing limits",
    "correct: crates/splinterm/src/wayland.rs:7675-7729 - bounded Replace applies through the graphical client update path"
  ],
  "manualNotes": "Accept the final no-go record. Slices 1-3 and correctness fallbacks are valid, Slice 4 no-change is justified, and beta1 remains forbidden."
}
```
