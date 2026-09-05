//! Experimental, test-only shaping of one compatible single-font Latin/LTR run.
//!
//! Callers supply adjacent logical leaders (not wide-cell spacers) and authoritative
//! terminal widths. Byte ranges always refer to the concatenated original text;
//! glyph count and advances never redefine terminal columns or source semantics.
//! Run itemization, other scripts/directions, style/fallback boundaries, cursor
//! policy, variation overrides, and integration with painting/cache invalidation
//! are deliberately outside this prototype. Source spans are not ink bounds or
//! contextual dependency spans: calt may retain separate one-cell clusters while
//! drawing across columns. No live renderer path uses this prototype.

use std::ops::Range;

use anyhow::{Result, ensure};
use splinterm_protocol::MAX_COLUMNS;
use swash::{
    FontRef, Setting,
    shape::{
        Direction, ShapeContext,
        cluster::{Glyph, GlyphCluster},
    },
    text::Script,
};

// Experimental resource ceilings, not new user configuration or protocol limits.
const MAX_RUN_BYTES: usize = 64 * 1024;
const MAX_FEATURES: usize = 64;
const MAX_OUTPUT_GLYPHS: usize = 64 * 1024;

#[derive(Clone, Copy, Debug)]
struct LogicalCell<'a> {
    text: &'a str,
    column: usize,
    width: usize,
}

/// Exact four-byte printable ASCII tags, case preserved, sorted with no duplicates.
/// Values are unsigned OpenType feature selectors, not just boolean switches.
#[derive(Debug)]
struct FeatureSettings(Vec<Setting<u16>>);

impl FeatureSettings {
    fn new(settings: &[(&str, i64)]) -> Result<Self> {
        ensure!(settings.len() <= MAX_FEATURES, "too many feature settings");
        let mut normalized = Vec::with_capacity(settings.len());
        for &(tag, value) in settings {
            ensure!(
                tag.len() == 4 && tag.bytes().all(|byte| (0x20..=0x7e).contains(&byte)),
                "feature tag must be exactly four printable ASCII bytes"
            );
            let value = u16::try_from(value)?;
            normalized.push(Setting {
                tag: u32::from_be_bytes(tag.as_bytes().try_into()?),
                value,
            });
        }
        normalized.sort_unstable_by_key(|setting| setting.tag);
        ensure!(
            normalized.windows(2).all(|pair| pair[0].tag != pair[1].tag),
            "duplicate feature tag"
        );
        Ok(Self(normalized))
    }
}

#[derive(Debug, PartialEq, Eq)]
struct CellSource {
    bytes: Range<usize>,
    columns: Range<usize>,
}

#[derive(Debug, PartialEq, Eq)]
struct SourceSpan {
    bytes: Range<usize>,
    /// Indices into the original logical leader sequence, not glyph indices.
    cells: Range<usize>,
    columns: Range<usize>,
}

#[derive(Debug)]
struct RunSource {
    text: String,
    cells: Vec<CellSource>,
}

impl RunSource {
    fn new(cells: &[LogicalCell<'_>], row_columns: usize) -> Result<Self> {
        ensure!(
            (1..=usize::from(MAX_COLUMNS)).contains(&row_columns),
            "row width is out of bounds"
        );
        ensure!(
            !cells.is_empty() && cells.len() <= row_columns,
            "run must contain a bounded nonempty sequence of logical cells"
        );
        let mut byte_len = 0_usize;
        let mut next_column = cells[0].column;
        for cell in cells {
            ensure!(
                !cell.text.is_empty(),
                "logical leader text must not be empty"
            );
            ensure!(cell.width > 0, "logical leader width must be positive");
            ensure!(cell.column == next_column, "logical cells must be adjacent");
            next_column = cell
                .column
                .checked_add(cell.width)
                .ok_or_else(|| anyhow::anyhow!("logical cell column overflow"))?;
            ensure!(next_column <= row_columns, "logical cell exceeds row width");
            byte_len = byte_len
                .checked_add(cell.text.len())
                .ok_or_else(|| anyhow::anyhow!("run byte length overflow"))?;
            ensure!(byte_len <= MAX_RUN_BYTES, "run byte limit exceeded");
        }
        let mut source = Self {
            text: String::with_capacity(byte_len),
            cells: Vec::with_capacity(cells.len()),
        };
        for cell in cells {
            let start = source.text.len();
            source.text.push_str(cell.text);
            source.cells.push(CellSource {
                bytes: start..source.text.len(),
                columns: cell.column..cell.column + cell.width,
            });
        }
        Ok(source)
    }

    /// Intersect source bytes with leaders; preserve each touched leader's full width.
    fn span(&self, bytes: Range<usize>) -> Result<SourceSpan> {
        ensure!(
            bytes.start < bytes.end
                && bytes.end <= self.text.len()
                && self.text.is_char_boundary(bytes.start)
                && self.text.is_char_boundary(bytes.end),
            "invalid cluster byte range"
        );
        let first = self
            .cells
            .partition_point(|cell| cell.bytes.end <= bytes.start);
        let end = self
            .cells
            .partition_point(|cell| cell.bytes.start < bytes.end);
        Ok(SourceSpan {
            bytes,
            cells: first..end,
            columns: self.cells[first].columns.start..self.cells[end - 1].columns.end,
        })
    }
}

#[derive(Debug)]
struct ShapedCluster {
    source: SourceSpan,
    components: Vec<SourceSpan>,
    /// Retain every glyph's ID, advance, offsets and flags, including zero advances.
    glyphs: Vec<Glyph>,
}

impl ShapedCluster {
    fn from_swash(source: &RunSource, cluster: &GlyphCluster<'_>) -> Result<Self> {
        let span = source.span(cluster.source.start as usize..cluster.source.end as usize)?;
        let mut components = Vec::new();
        for component in cluster.components {
            ensure!(
                component.start >= cluster.source.start && component.end <= cluster.source.end,
                "ligature component outside cluster"
            );
            components.push(source.span(component.start as usize..component.end as usize)?);
        }
        Ok(Self {
            source: span,
            components,
            glyphs: cluster.glyphs.to_vec(),
        })
    }
}

#[derive(Debug)]
struct ShapedRun {
    source: RunSource,
    clusters: Vec<ShapedCluster>,
}

fn shape_run(
    context: &mut ShapeContext,
    font: FontRef<'_>,
    cells: &[LogicalCell<'_>],
    row_columns: usize,
    size: f32,
    features: &FeatureSettings,
) -> Result<ShapedRun> {
    ensure!(
        size.is_finite() && size > 0.0 && size <= 1024.0,
        "invalid font size"
    );
    let source = RunSource::new(cells, row_columns)?;
    let mut shaper = context
        .builder(font)
        .script(Script::Latin)
        .direction(Direction::LeftToRight)
        .size(size)
        .features(features.0.iter().copied())
        .build();
    // One add_str call is important: Swash's byte offsets restart with each call.
    shaper.add_str(&source.text);
    let mut clusters = Vec::new();
    let mut glyph_count = 0_usize;
    let mut result = Ok(());
    shaper.shape_with(|cluster| {
        if result.is_err() {
            return;
        }
        result = (|| {
            ensure!(
                clusters.len() < MAX_RUN_BYTES,
                "cluster output limit exceeded"
            );
            glyph_count = glyph_count
                .checked_add(cluster.glyphs.len())
                .ok_or_else(|| anyhow::anyhow!("glyph count overflow"))?;
            ensure!(
                glyph_count <= MAX_OUTPUT_GLYPHS,
                "glyph output limit exceeded"
            );
            clusters.push(ShapedCluster::from_swash(&source, cluster)?);
            Ok(())
        })();
    });
    result?;
    Ok(ShapedRun { source, clusters })
}

#[cfg(test)]
mod tests {
    use std::{fs, process::Command, sync::OnceLock};

    use sha2::{Digest as _, Sha256};

    use super::*;

    fn pinned_font() -> FontRef<'static> {
        static DATA: OnceLock<Vec<u8>> = OnceLock::new();
        let data = DATA.get_or_init(|| {
            let output = Command::new("fc-match")
                .args(["--format=%{file}", "JetBrains Mono Nerd Font:style=Regular"])
                .output()
                .expect("fc-match must be available for the pinned CI renderer font");
            assert!(output.status.success(), "fc-match failed: {output:?}");
            let path = std::str::from_utf8(&output.stdout).unwrap();
            let data = fs::read(path).expect("read pinned renderer font");
            assert_eq!(
                format!("{:x}", Sha256::digest(&data)),
                "0ec29a68b539ece7078fc714cebff0c0accb2f4948f8f7963d9f5e86633b12d9",
                "requires the exact Regular font installed by .github/workflows/ci.yml, got {path}"
            );
            data
        });
        FontRef::from_index(data, 0).expect("valid pinned font")
    }

    fn cells<'a>(texts: &[&'a str]) -> Vec<LogicalCell<'a>> {
        texts
            .iter()
            .enumerate()
            .map(|(column, text)| LogicalCell {
                text,
                column,
                width: 1,
            })
            .collect()
    }

    fn shape(cells: &[LogicalCell<'_>], calt: i64) -> ShapedRun {
        shape_run(
            &mut ShapeContext::new(),
            pinned_font(),
            cells,
            usize::from(MAX_COLUMNS),
            20.0,
            &FeatureSettings::new(&[("calt", calt)]).unwrap(),
        )
        .unwrap()
    }

    fn glyph_ids(run: &ShapedRun) -> Vec<u16> {
        run.clusters
            .iter()
            .flat_map(|cluster| cluster.glyphs.iter().map(|glyph| glyph.id))
            .collect()
    }

    #[test]
    fn shaping_ascii_nonligatures_preserve_columns_and_text() {
        let run = shape(&cells(&["a", "b", "c"]), 1);
        assert_eq!(run.source.text, "abc");
        assert_eq!(run.clusters.len(), 3);
        for (index, cluster) in run.clusters.iter().enumerate() {
            let end = index + 1;
            assert_eq!(cluster.source.bytes, index..end);
            assert_eq!(cluster.source.cells, index..end);
            assert_eq!(cluster.source.columns, index..end);
            assert!(cluster.components.is_empty());
            assert_eq!(cluster.glyphs.len(), 1);
            assert!(cluster.glyphs[0].advance > 0.0);
        }
    }

    #[test]
    fn shaping_calt_changes_real_adjacent_cell_glyphs_not_source_semantics() {
        // Contextual substitutions need not collapse into a single ligature glyph.
        for texts in [&["!", "="][..], &["=", "=", "="], &["-", ">"]] {
            let input = cells(texts);
            let enabled = shape(&input, 1);
            let disabled = shape(&input, 0);
            assert_ne!(glyph_ids(&enabled), glyph_ids(&disabled), "{texts:?}");
            assert_eq!(enabled.source.text, texts.concat());
            assert_eq!(enabled.source.text, disabled.source.text);
            assert_eq!(enabled.source.cells, disabled.source.cells);
            let individually_shaped: Vec<_> = texts
                .iter()
                .flat_map(|text| glyph_ids(&shape(&cells(&[text]), 1)))
                .collect();
            assert_ne!(
                glyph_ids(&enabled),
                individually_shaped,
                "requires run context"
            );
            assert_eq!(enabled.clusters.len(), texts.len());
            for (index, cluster) in enabled.clusters.iter().enumerate() {
                // This authority uses contextual replacements, not merged clusters.
                let end = index + 1;
                assert_eq!(cluster.source.bytes, index..end);
                assert_eq!(cluster.source.cells, index..end);
                assert_eq!(cluster.source.columns, index..end);
                assert!(cluster.components.is_empty());
                assert_eq!(cluster.glyphs.len(), 1);
                assert_ne!(cluster.glyphs[0].id, 0);
            }
        }
    }

    #[test]
    fn shaping_unicode_byte_ranges_combining_and_wide_leaders() {
        let input = [
            LogicalCell {
                text: "a\u{301}\u{323}",
                column: 4,
                width: 1,
            },
            LogicalCell {
                text: "é",
                column: 5,
                width: 2,
            },
            LogicalCell {
                text: "z",
                column: 7,
                width: 1,
            },
        ];
        let run = shape(&input, 0);
        assert_eq!(run.source.text, "a\u{301}\u{323}éz");
        assert_eq!(run.source.cells[0].bytes, 0..5);
        assert_eq!(run.source.cells[1].bytes, 5..7);
        assert_eq!(run.clusters[0].source.bytes, 0..5);
        assert_eq!(run.clusters[0].source.columns, 4..5);
        assert!(
            run.clusters[0].glyphs.len() > 1,
            "base plus uncomposed mark"
        );
        assert_eq!(run.clusters[1].source.bytes, 5..7);
        assert_eq!(run.clusters[1].source.columns, 5..7);
        assert_eq!(
            run.source.span(1..7).unwrap(),
            SourceSpan {
                bytes: 1..7,
                cells: 0..2,
                columns: 4..7,
            }
        );
        for glyph in run.clusters.iter().flat_map(|cluster| &cluster.glyphs) {
            assert!(glyph.id != 0, "no missing-glyph substitution");
            assert!(glyph.x.is_finite() && glyph.y.is_finite() && glyph.advance.is_finite());
        }
    }

    #[test]
    fn shaping_empty_glyph_cluster_keeps_its_source_cell() {
        let run = shape(&cells(&["a", "\n", "b"]), 0);
        let empty = run
            .clusters
            .iter()
            .find(|cluster| cluster.glyphs.is_empty())
            .unwrap();
        assert_eq!(empty.source.bytes, 1..2);
        assert_eq!(empty.source.cells, 1..2);
        assert_eq!(empty.source.columns, 1..2);
        assert_eq!(run.source.text, "a\nb");
    }

    #[test]
    fn shaping_source_spans_can_cover_multiple_cells_and_partial_leaders() {
        let source = RunSource::new(&cells(&["!", "=", "a\u{301}"]), 3).unwrap();
        assert_eq!(
            source.span(0..2).unwrap(),
            SourceSpan {
                bytes: 0..2,
                cells: 0..2,
                columns: 0..2,
            }
        );
        assert_eq!(
            source.span(3..5).unwrap(),
            SourceSpan {
                bytes: 3..5,
                cells: 2..3,
                columns: 2..3,
            }
        );
        for (start, end) in [
            (0, 0),
            (2, 1),
            (0, 6),
            (3, 4),
            (4, 5),
            (usize::MAX, usize::MAX),
        ] {
            assert!(source.span(start..end).is_err());
        }
    }

    #[test]
    fn shaping_merged_ligature_components_keep_original_cell_spans() {
        use swash::text::cluster::{ClusterInfo, SourceRange};

        // JetBrains Mono's calt sequences above use separate clusters. Exercise
        // the merged-cluster output contract directly without another font oracle.
        let source = RunSource::new(
            &[
                LogicalCell {
                    text: "!",
                    column: 3,
                    width: 2,
                },
                LogicalCell {
                    text: "=",
                    column: 5,
                    width: 1,
                },
            ],
            6,
        )
        .unwrap();
        let glyphs = [Glyph {
            id: 42,
            advance: 24.0,
            x: -2.0,
            y: 3.0,
            ..Glyph::default()
        }];
        let components = [
            SourceRange { start: 0, end: 1 },
            SourceRange { start: 1, end: 2 },
        ];
        let cluster = GlyphCluster {
            source: SourceRange { start: 0, end: 2 },
            info: ClusterInfo::default(),
            glyphs: &glyphs,
            components: &components,
            data: 0,
        };
        let mapped = ShapedCluster::from_swash(&source, &cluster).unwrap();
        assert_eq!(
            mapped.source,
            SourceSpan {
                bytes: 0..2,
                cells: 0..2,
                columns: 3..6
            }
        );
        assert_eq!(
            mapped.components,
            vec![
                SourceSpan {
                    bytes: 0..1,
                    cells: 0..1,
                    columns: 3..5
                },
                SourceSpan {
                    bytes: 1..2,
                    cells: 1..2,
                    columns: 5..6
                },
            ]
        );
        assert_eq!(mapped.glyphs.len(), 1);
        assert_eq!(mapped.glyphs[0].id, 42);
        assert_eq!(
            mapped.glyphs[0].advance.to_bits(),
            glyphs[0].advance.to_bits()
        );
        assert_eq!(mapped.glyphs[0].x.to_bits(), glyphs[0].x.to_bits());
        assert_eq!(mapped.glyphs[0].y.to_bits(), glyphs[0].y.to_bits());
        let outside = [SourceRange { start: 0, end: 3 }];
        assert!(
            ShapedCluster::from_swash(
                &source,
                &GlyphCluster {
                    components: &outside,
                    ..cluster
                }
            )
            .is_err()
        );
        let empty = [SourceRange { start: 1, end: 1 }];
        assert!(
            ShapedCluster::from_swash(
                &source,
                &GlyphCluster {
                    components: &empty,
                    ..cluster
                }
            )
            .is_err()
        );
    }

    #[test]
    fn shaping_feature_settings_are_validated_and_normalized() {
        let features = FeatureSettings::new(&[("ss01", 65535), ("calt", 0), ("liga", 1)]).unwrap();
        assert_eq!(
            features.0,
            vec![
                Setting::from(("calt", 0)),
                Setting::from(("liga", 1)),
                Setting::from(("ss01", 65535)),
            ]
        );
        for tag in ["", "cal", "caltt", "cált", "cal\n", "\0abc"] {
            assert!(FeatureSettings::new(&[(tag, 1)]).is_err());
        }
        for value in [-1, 65536, i64::MAX] {
            assert!(FeatureSettings::new(&[("calt", value)]).is_err());
        }
        assert!(FeatureSettings::new(&[("calt", 0), ("calt", 1)]).is_err());
        assert!(FeatureSettings::new(&vec![("calt", 1); MAX_FEATURES + 1]).is_err());
        assert!(FeatureSettings::new(&[]).unwrap().0.is_empty());
        assert_eq!(
            FeatureSettings::new(&[("CALt", 1)]).unwrap().0[0].tag,
            u32::from_be_bytes(*b"CALt")
        );
    }

    #[test]
    fn shaping_rejects_invalid_and_unbounded_inputs() {
        let valid = cells(&["a"]);
        for columns in [0, usize::from(MAX_COLUMNS) + 1, usize::MAX] {
            assert!(RunSource::new(&valid, columns).is_err());
        }
        assert!(RunSource::new(&[], 1).is_err());
        assert!(RunSource::new(&cells(&["a", "b"]), 1).is_err());
        for (text, column, width) in [("", 0, 1), ("a", 0, 0), ("a", 0, 2), ("a", usize::MAX, 1)] {
            assert!(
                RunSource::new(
                    &[LogicalCell {
                        text,
                        column,
                        width
                    }],
                    1
                )
                .is_err()
            );
        }
        for column in [0, 2] {
            assert!(
                RunSource::new(
                    &[
                        valid[0],
                        LogicalCell {
                            text: "b",
                            column,
                            width: 1
                        },
                    ],
                    3
                )
                .is_err(),
                "reject overlap and gap"
            );
        }
        let at_limit = "a".repeat(MAX_RUN_BYTES);
        assert!(RunSource::new(&cells(&[&at_limit]), 1).is_ok());
        assert!(RunSource::new(&cells(&[&(at_limit + "a")]), 1).is_err());
        let features = FeatureSettings::new(&[]).unwrap();
        for size in [0.0, -1.0, f32::NAN, f32::INFINITY, 1025.0] {
            assert!(
                shape_run(
                    &mut ShapeContext::new(),
                    pinned_font(),
                    &valid,
                    1,
                    size,
                    &features
                )
                .is_err()
            );
        }
    }
}
