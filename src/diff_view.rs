//! Display-row projection of a parsed patch.
//!
//! The projection is the single index used by diff rendering and navigation.
//! Every display row addresses the raw patch: the canonical raw row (whose
//! index is the GitHub line-comment position), the wrapped segment of that
//! row and the character offset at which the segment starts. It never
//! synthesizes content; rendering slices the original patch text.
//!
//! The unified projection shows one column. The split projection shows the
//! old side on the left and the new side on the right: context rows appear on
//! both sides, and within a hunk a maximal run of deletions immediately
//! followed by additions is paired by position. Pairing is a visual aid only;
//! each side keeps its own canonical raw row, and alignment filler has none.

use std::ops::Range;

use crate::inbox::{DiffLine, DiffLineKind};
use crate::ui_layout::for_each_wrap_segment;

/// How the diff pane lays out a patch. The effective mode is part of the
/// projection cache key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DiffViewMode {
    Unified,
    #[default]
    Split,
}

/// The side of a split row that line actions target.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DiffSide {
    Old,
    #[default]
    New,
}

impl DiffSide {
    pub fn label(self) -> &'static str {
        match self {
            Self::Old => "old",
            Self::New => "new",
        }
    }

    pub fn other(self) -> Self {
        match self {
            Self::Old => Self::New,
            Self::New => Self::Old,
        }
    }
}

/// Minimum text columns per side, after each side's number gutter, for the
/// split view to be readable. Narrower diff panes fall back to unified.
pub const MIN_SPLIT_SIDE_TEXT_WIDTH: usize = 20;

/// Columns of the vertical separator between the old and new sides.
pub const SPLIT_SEPARATOR_WIDTH: usize = 1;

/// Column widths of a split projection within the diff pane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SplitLayout {
    /// Per-side gutter: draft/comment marker and that side's line number.
    pub side_gutter: usize,
    /// Text columns of each side.
    pub side_text: usize,
    /// Text columns of full-width rows (hunk headers, unsupported rows).
    pub full_text: usize,
}

impl SplitLayout {
    /// The split geometry for `inner_width` pane columns, or `None` when a
    /// side would have fewer than `MIN_SPLIT_SIDE_TEXT_WIDTH` text columns.
    pub fn fit(lines: &[DiffLine], inner_width: usize) -> Option<Self> {
        let side_gutter = max_line_digits(lines).saturating_add(3);
        let side_text = inner_width.checked_sub(SPLIT_SEPARATOR_WIDTH + 2 * side_gutter)? / 2;
        (side_text >= MIN_SPLIT_SIDE_TEXT_WIDTH).then_some(Self {
            side_gutter,
            side_text,
            full_text: inner_width.saturating_sub(side_gutter).max(1),
        })
    }

    /// Columns of one side including its gutter.
    pub fn side_width(self) -> usize {
        self.side_gutter + self.side_text
    }
}

fn max_line_digits(lines: &[DiffLine]) -> usize {
    let max_number = lines
        .iter()
        .flat_map(|line| [line.old_line, line.new_line])
        .flatten()
        .max()
        .unwrap_or(0);
    max_number.to_string().len().max(1)
}

/// A logical reading position that survives rebuilds at other widths and in
/// the other view mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiffAnchor {
    pub raw_row: usize,
    pub char_offset: usize,
}

/// One wrapped piece of a raw patch row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Segment {
    /// Index of the patch row this segment shows.
    pub raw_row: usize,
    /// Wrapped segment index within the raw row; 0 for the first row.
    pub segment: usize,
    /// Character offset of the segment start within the raw row text.
    pub char_offset: usize,
    /// Byte range of the segment within the raw row text.
    pub bytes: Range<usize>,
    pub kind: DiffLineKind,
}

impl Segment {
    pub fn is_continuation(&self) -> bool {
        self.segment > 0
    }

    /// Raw row a line comment would target, if the row kind can carry one.
    /// Continuations carry the original raw row; hunk headers, no-newline
    /// markers and unsupported rows carry none. Eligibility is still decided
    /// by `comment_draft::line_target`. A genuinely empty source line is a
    /// real segment and keeps its target.
    pub fn comment_row(&self) -> Option<usize> {
        matches!(
            self.kind,
            DiffLineKind::Context | DiffLineKind::Addition | DiffLineKind::Deletion
        )
        .then_some(self.raw_row)
    }
}

/// One side of a split display row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SideCell {
    /// A displayed segment of a patch row on this side. Its text may be
    /// genuinely empty.
    Content(Segment),
    /// Visual alignment padding: a missing side of a pure addition or
    /// deletion, the shorter side of an unequal block, or blank rows below a
    /// shorter wrapped line. Filler never has a comment target.
    Filler,
}

impl SideCell {
    pub fn segment(&self) -> Option<&Segment> {
        match self {
            Self::Content(segment) => Some(segment),
            Self::Filler => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DisplayRow {
    /// A unified row, or a full-width split row (hunk header, unsupported or
    /// unattached row). It targets the same raw row regardless of side.
    Single(Segment),
    /// A split row with independent old and new cells.
    Split { old: SideCell, new: SideCell },
}

impl DisplayRow {
    /// The segment shown on `side`; single rows span both sides.
    pub fn segment(&self, side: DiffSide) -> Option<&Segment> {
        match self {
            Self::Single(segment) => Some(segment),
            Self::Split { old, new } => match side {
                DiffSide::Old => old.segment(),
                DiffSide::New => new.segment(),
            },
        }
    }

    /// The segment that represents the row when a side does not matter: the
    /// new side when it has content, otherwise the old side.
    pub fn primary(&self) -> &Segment {
        match self {
            Self::Single(segment) => segment,
            Self::Split { old, new } => new
                .segment()
                .or_else(|| old.segment())
                .expect("every split row shows content on at least one side"),
        }
    }

    /// The segment on `side`, falling back to the other side for filler.
    pub fn segment_or_other(&self, side: DiffSide) -> &Segment {
        self.segment(side)
            .or_else(|| self.segment(side.other()))
            .unwrap_or_else(|| self.primary())
    }

    /// Raw row a line comment on `side` would target. Filler, headers and
    /// markers have none.
    pub fn comment_row(&self, side: DiffSide) -> Option<usize> {
        self.segment(side).and_then(Segment::comment_row)
    }
}

/// Where a raw row's segments are displayed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Placement {
    first: usize,
    count: usize,
    /// The side holding the row in split mode; `None` for single rows and
    /// for context rows shown identically on both sides.
    side: Option<DiffSide>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffProjection {
    mode: DiffViewMode,
    rows: Vec<DisplayRow>,
    /// Display rows of every raw row, indexed by raw row.
    placements: Vec<Placement>,
    hunk_starts: Vec<usize>,
    /// Unified: the full gutter. Split: the per-side gutter.
    gutter_width: usize,
    split: Option<SplitLayout>,
}

impl DiffProjection {
    /// Builds the unified projection in one pass over the patch.
    /// `content_width` is the number of terminal columns available to patch
    /// text.
    pub fn build(lines: &[DiffLine], content_width: usize, gutter_width: usize) -> Self {
        let mut rows = Vec::with_capacity(lines.len());
        let mut placements = Vec::with_capacity(lines.len());
        let mut hunk_starts = Vec::new();
        for (raw_row, line) in lines.iter().enumerate() {
            let first = rows.len();
            if line.kind == DiffLineKind::Hunk {
                hunk_starts.push(first);
            }
            for_each_segment(raw_row, line, content_width, |segment| {
                rows.push(DisplayRow::Single(segment));
            });
            placements.push(Placement {
                first,
                count: rows.len() - first,
                side: None,
            });
        }
        Self {
            mode: DiffViewMode::Unified,
            rows,
            placements,
            hunk_starts,
            gutter_width,
            split: None,
        }
    }

    /// Builds the split projection in one pass over the patch.
    pub fn build_split(lines: &[DiffLine], layout: SplitLayout) -> Self {
        let mut builder = SplitBuilder {
            lines,
            layout,
            rows: Vec::with_capacity(lines.len()),
            placements: vec![
                Placement {
                    first: 0,
                    count: 0,
                    side: None,
                };
                lines.len()
            ],
            hunk_starts: Vec::new(),
        };
        let mut index = 0;
        while index < lines.len() {
            index = match lines[index].kind {
                DiffLineKind::Context => builder.context(index),
                DiffLineKind::Deletion | DiffLineKind::Addition => builder.change_run(index),
                DiffLineKind::Hunk | DiffLineKind::NoNewline | DiffLineKind::Other => {
                    builder.single(index)
                }
            };
        }
        Self {
            mode: DiffViewMode::Split,
            rows: builder.rows,
            placements: builder.placements,
            hunk_starts: builder.hunk_starts,
            gutter_width: layout.side_gutter,
            split: Some(layout),
        }
    }

    /// Gutter width for a unified patch: draft/comment marker plus old and
    /// new line numbers sized to the largest number present.
    pub fn gutter_width_for(lines: &[DiffLine]) -> usize {
        max_line_digits(lines).saturating_mul(2).saturating_add(5)
    }

    pub fn mode(&self) -> DiffViewMode {
        self.mode
    }

    pub fn split_layout(&self) -> Option<SplitLayout> {
        self.split
    }

    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    pub fn gutter_width(&self) -> usize {
        self.gutter_width
    }

    pub fn row(&self, index: usize) -> Option<&DisplayRow> {
        self.rows.get(index)
    }

    /// Display rows in `start..start + count`, clamped to the projection.
    pub fn rows(&self, start: usize, count: usize) -> &[DisplayRow] {
        let start = start.min(self.rows.len());
        let end = start.saturating_add(count).min(self.rows.len());
        &self.rows[start..end]
    }

    /// Text shown by a segment, sliced from the original patch line.
    pub fn row_text<'a>(&self, lines: &'a [DiffLine], segment: &Segment) -> &'a str {
        lines
            .get(segment.raw_row)
            .and_then(|line| line.text.get(segment.bytes.clone()))
            .unwrap_or("")
    }

    /// Display rows occupied by a raw row's own segments.
    pub fn raw_row_rows(&self, raw_row: usize) -> Range<usize> {
        self.placements.get(raw_row).map_or(0..0, |placement| {
            placement.first..placement.first + placement.count
        })
    }

    /// The split side that holds a raw row: old for deletions and markers
    /// attached to them, new for additions and their markers, `None` for rows
    /// shown on both sides or full width, and always `None` in unified mode.
    pub fn raw_row_side(&self, raw_row: usize) -> Option<DiffSide> {
        self.placements
            .get(raw_row)
            .and_then(|placement| placement.side)
    }

    /// Display row that contains `anchor`. A raw row beyond the patch clamps
    /// to the last raw row; an offset beyond the text clamps to its last
    /// segment.
    pub fn row_for_anchor(&self, anchor: DiffAnchor) -> Option<usize> {
        let raw_row = anchor.raw_row.min(self.placements.len().checked_sub(1)?);
        let placement = self.placements[raw_row];
        let side = placement.side.unwrap_or(DiffSide::Old);
        let range = self.raw_row_rows(raw_row);
        let within = self.rows[range.clone()]
            .partition_point(|row| {
                row.segment(side)
                    .is_some_and(|segment| segment.char_offset <= anchor.char_offset)
            })
            .saturating_sub(1);
        Some(range.start + within)
    }

    /// The logical position of display row `index` as read on `side`. Filler
    /// on `side` reads the other side's segment on the same row.
    pub fn anchor(&self, index: usize, side: DiffSide) -> Option<DiffAnchor> {
        self.rows.get(index).map(|row| {
            let segment = row.segment_or_other(side);
            DiffAnchor {
                raw_row: segment.raw_row,
                char_offset: segment.char_offset,
            }
        })
    }

    #[cfg(test)]
    pub fn hunk_starts(&self) -> &[usize] {
        &self.hunk_starts
    }

    pub fn hunk_count(&self) -> usize {
        self.hunk_starts.len()
    }

    /// One-based index of the hunk containing `index`; 0 before the first
    /// hunk header.
    pub fn hunk_number(&self, index: usize) -> usize {
        self.hunk_starts.partition_point(|start| *start <= index)
    }

    pub fn next_hunk(&self, index: usize) -> Option<usize> {
        let position = self.hunk_starts.partition_point(|start| *start <= index);
        self.hunk_starts.get(position).copied()
    }

    pub fn previous_hunk(&self, index: usize) -> Option<usize> {
        let position = self.hunk_starts.partition_point(|start| *start < index);
        position
            .checked_sub(1)
            .and_then(|position| self.hunk_starts.get(position).copied())
    }
}

fn for_each_segment(raw_row: usize, line: &DiffLine, width: usize, mut visit: impl FnMut(Segment)) {
    let mut segment = 0;
    for_each_wrap_segment(&line.text, width, |wrapped| {
        visit(Segment {
            raw_row,
            segment,
            char_offset: wrapped.char_start,
            bytes: wrapped.bytes,
            kind: line.kind,
        });
        segment += 1;
    });
}

struct SplitBuilder<'a> {
    lines: &'a [DiffLine],
    layout: SplitLayout,
    rows: Vec<DisplayRow>,
    placements: Vec<Placement>,
    hunk_starts: Vec<usize>,
}

impl SplitBuilder<'_> {
    /// A full-width row: hunk header, unsupported row or a no-newline marker
    /// with no preceding line to attach to.
    fn single(&mut self, raw_row: usize) -> usize {
        let first = self.rows.len();
        let line = &self.lines[raw_row];
        if line.kind == DiffLineKind::Hunk {
            self.hunk_starts.push(first);
        }
        let rows = &mut self.rows;
        for_each_segment(raw_row, line, self.layout.full_text, |segment| {
            rows.push(DisplayRow::Single(segment));
        });
        self.placements[raw_row] = Placement {
            first,
            count: self.rows.len() - first,
            side: None,
        };
        raw_row + 1
    }

    /// The raw rows of a line and the no-newline markers attached to it.
    fn attached(&self, raw_row: usize) -> Range<usize> {
        let mut end = raw_row + 1;
        while self
            .lines
            .get(end)
            .is_some_and(|line| line.kind == DiffLineKind::NoNewline)
        {
            end += 1;
        }
        raw_row..end
    }

    /// Wrapped segments of a line and its attached markers at side width.
    fn side_segments(&self, raw_rows: Range<usize>) -> Vec<Segment> {
        let mut segments = Vec::new();
        for raw_row in raw_rows {
            for_each_segment(
                raw_row,
                &self.lines[raw_row],
                self.layout.side_text,
                |segment| segments.push(segment),
            );
        }
        segments
    }

    /// Records where each raw row of a side column starts in display rows.
    fn place(&mut self, first: usize, segments: &[Segment], side: Option<DiffSide>) {
        for (offset, segment) in segments.iter().enumerate() {
            let placement = &mut self.placements[segment.raw_row];
            if segment.segment == 0 {
                *placement = Placement {
                    first: first + offset,
                    count: 0,
                    side,
                };
            }
            placement.count += 1;
        }
    }

    /// A context line (with its markers) shown on both sides.
    fn context(&mut self, raw_row: usize) -> usize {
        let raw_rows = self.attached(raw_row);
        let end = raw_rows.end;
        let segments = self.side_segments(raw_rows);
        self.place(self.rows.len(), &segments, None);
        self.rows
            .extend(segments.into_iter().map(|segment| DisplayRow::Split {
                old: SideCell::Content(segment.clone()),
                new: SideCell::Content(segment),
            }));
        end
    }

    /// A maximal run of deletions followed by additions, paired by position.
    /// Attached no-newline markers travel with their line and do not end the
    /// run. Hunk headers, context and other rows always end it.
    fn change_run(&mut self, start: usize) -> usize {
        let mut index = start;
        let mut deletions = Vec::new();
        while self
            .lines
            .get(index)
            .is_some_and(|line| line.kind == DiffLineKind::Deletion)
        {
            let raw_rows = self.attached(index);
            index = raw_rows.end;
            deletions.push(self.side_segments(raw_rows));
        }
        let mut additions = Vec::new();
        while self
            .lines
            .get(index)
            .is_some_and(|line| line.kind == DiffLineKind::Addition)
        {
            let raw_rows = self.attached(index);
            index = raw_rows.end;
            additions.push(self.side_segments(raw_rows));
        }
        let pairs = deletions.len().max(additions.len());
        let mut deletions = deletions.into_iter();
        let mut additions = additions.into_iter();
        for _ in 0..pairs {
            let old = deletions.next().unwrap_or_default();
            let new = additions.next().unwrap_or_default();
            let first = self.rows.len();
            self.place(first, &old, Some(DiffSide::Old));
            self.place(first, &new, Some(DiffSide::New));
            let height = old.len().max(new.len());
            let mut old = old.into_iter();
            let mut new = new.into_iter();
            for _ in 0..height {
                self.rows.push(DisplayRow::Split {
                    old: old.next().map_or(SideCell::Filler, SideCell::Content),
                    new: new.next().map_or(SideCell::Filler, SideCell::Content),
                });
            }
        }
        index
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::github::parse_patch_text;
    use crate::ui_layout::wrap_text;

    fn lines(patch: &str) -> Vec<DiffLine> {
        parse_patch_text(patch).lines().to_vec()
    }

    fn texts(projection: &DiffProjection, lines: &[DiffLine]) -> Vec<String> {
        projection
            .rows(0, projection.len())
            .iter()
            .map(|row| projection.row_text(lines, row.primary()).to_owned())
            .collect()
    }

    #[test]
    fn unwrapped_rows_map_one_to_one_with_kinds_and_comment_rows() {
        let lines = lines("@@ -1,2 +1,2 @@\n context\n-old\n+new\n\\ No newline at end of file");
        let projection = DiffProjection::build(&lines, 80, 7);

        assert_eq!(projection.len(), lines.len());
        assert_eq!(projection.hunk_starts(), &[0]);
        let comment_rows: Vec<_> = projection
            .rows(0, 10)
            .iter()
            .map(|row| row.comment_row(DiffSide::New))
            .collect();
        assert_eq!(comment_rows, vec![None, Some(1), Some(2), Some(3), None]);
        assert_eq!(
            projection.row(4).unwrap().primary().kind,
            DiffLineKind::NoNewline
        );
        assert_eq!(
            texts(&projection, &lines),
            lines
                .iter()
                .map(|line| line.text.clone())
                .collect::<Vec<_>>()
        );
        assert_eq!(projection.gutter_width(), 7);
    }

    #[test]
    fn wrapped_continuations_keep_the_original_raw_row_and_offsets() {
        let lines = lines("@@ -1 +1 @@\n+abcdefghijklmnopqrstuvwxy\n x");
        let projection = DiffProjection::build(&lines, 11, 7);

        // The addition is 26 characters: rows of 11, 11 and 4.
        assert_eq!(projection.len(), 1 + 3 + 1);
        let added: Vec<_> = projection
            .rows(1, 3)
            .iter()
            .map(|row| row.primary().clone())
            .collect();
        assert!(added.iter().all(|row| row.raw_row == 1));
        assert!(added.iter().all(|row| row.comment_row() == Some(1)));
        assert_eq!(
            added.iter().map(|row| row.segment).collect::<Vec<_>>(),
            vec![0, 1, 2]
        );
        assert_eq!(
            added.iter().map(|row| row.char_offset).collect::<Vec<_>>(),
            vec![0, 11, 22]
        );
        assert!(!added[0].is_continuation());
        assert!(added[1].is_continuation());
        assert_eq!(projection.raw_row_rows(2), 4..5);
        assert_eq!(projection.raw_row_rows(1), 1..4);
        assert_eq!(projection.raw_row_rows(3), 0..0);
        assert_eq!(
            texts(&projection, &lines),
            vec!["@@ -1 +1 @@", "+abcdefghij", "klmnopqrstu", "vwxy", " x"]
        );
    }

    #[test]
    fn wrapping_matches_rendering_for_unicode_widths() {
        let text = "+日本語のテキスト é zero\u{200b}width 👩‍💻 done";
        let lines = vec![DiffLine {
            kind: DiffLineKind::Addition,
            old_line: None,
            new_line: Some(1),
            text: text.to_owned(),
        }];
        for width in [1, 2, 3, 5, 8, 13, 80] {
            let projection = DiffProjection::build(&lines, width, 7);
            assert_eq!(
                texts(&projection, &lines),
                wrap_text(text, width),
                "width {width}"
            );
            for row in projection.rows(0, projection.len()) {
                let row = row.primary();
                assert_eq!(
                    text[..row.bytes.start].chars().count(),
                    row.char_offset,
                    "width {width}"
                );
            }
        }
    }

    #[test]
    fn empty_lines_still_occupy_one_row() {
        let lines = lines("@@ -1,2 +1,2 @@\n \n+");
        let projection = DiffProjection::build(&lines, 20, 7);
        assert_eq!(projection.len(), 3);
        assert_eq!(
            projection.row(1).unwrap().comment_row(DiffSide::New),
            Some(1)
        );
        assert_eq!(
            projection.row(2).unwrap().comment_row(DiffSide::Old),
            Some(2)
        );
    }

    #[test]
    fn hunk_starts_support_numbering_and_boundaries() {
        let lines = lines(
            "@@ -1 +1 @@\n a\n+b\n@@ -10 +10 @@\n c\n-d\n@@ -20 +20 @@\n e\n\\ No newline at end of file",
        );
        let projection = DiffProjection::build(&lines, 80, 9);

        assert_eq!(projection.hunk_starts(), &[0, 3, 6]);
        assert_eq!(projection.hunk_count(), 3);
        assert_eq!(projection.hunk_number(0), 1);
        assert_eq!(projection.hunk_number(2), 1);
        assert_eq!(projection.hunk_number(3), 2);
        assert_eq!(projection.hunk_number(8), 3);
        assert_eq!(projection.next_hunk(0), Some(3));
        assert_eq!(projection.next_hunk(4), Some(6));
        assert_eq!(projection.next_hunk(6), None);
        assert_eq!(projection.previous_hunk(7), Some(6));
        assert_eq!(projection.previous_hunk(6), Some(3));
        assert_eq!(projection.previous_hunk(0), None);
    }

    #[test]
    fn malformed_patch_without_hunks_has_no_hunk_starts() {
        let lines = lines("not a hunk\nstill not");
        let projection = DiffProjection::build(&lines, 80, 7);
        assert_eq!(projection.hunk_count(), 0);
        assert_eq!(projection.hunk_number(1), 0);
        assert_eq!(projection.next_hunk(0), None);
        assert!(
            projection
                .rows(0, 2)
                .iter()
                .all(|row| row.comment_row(DiffSide::New).is_none())
        );
    }

    #[test]
    fn anchors_round_trip_across_widths() {
        let lines = lines("@@ -1 +1 @@\n+0123456789abcdefghij\n z");
        let narrow = DiffProjection::build(&lines, 5, 7);
        let wide = DiffProjection::build(&lines, 8, 7);

        for index in 0..narrow.len() {
            let anchor = narrow.anchor(index, DiffSide::New).unwrap();
            assert_eq!(narrow.row_for_anchor(anchor), Some(index));
            let mapped = wide.row_for_anchor(anchor).unwrap();
            let row = wide.row(mapped).unwrap().primary();
            assert_eq!(row.raw_row, anchor.raw_row);
            let end = row.char_offset + projection_row_chars(&wide, &lines, mapped);
            assert!(row.char_offset <= anchor.char_offset);
            assert!(anchor.char_offset < end.max(row.char_offset + 1));
        }

        // Out-of-range anchors clamp instead of failing.
        assert_eq!(
            wide.row_for_anchor(DiffAnchor {
                raw_row: 99,
                char_offset: 0
            }),
            Some(wide.len() - 1)
        );
        assert_eq!(
            wide.row_for_anchor(DiffAnchor {
                raw_row: 1,
                char_offset: 999
            }),
            Some(wide.raw_row_rows(1).end - 1)
        );
        let empty = DiffProjection::build(&[], 8, 7);
        assert!(empty.is_empty());
        assert_eq!(
            empty.row_for_anchor(DiffAnchor {
                raw_row: 0,
                char_offset: 0
            }),
            None
        );
    }

    fn projection_row_chars(
        projection: &DiffProjection,
        lines: &[DiffLine],
        index: usize,
    ) -> usize {
        let row = projection.row(index).unwrap().primary();
        projection.row_text(lines, row).chars().count()
    }

    fn layout(side_text: usize) -> SplitLayout {
        SplitLayout {
            side_gutter: 4,
            side_text,
            full_text: 2 * side_text + 5,
        }
    }

    fn split(patch: &str, side_text: usize) -> (Vec<DiffLine>, DiffProjection) {
        let lines = lines(patch);
        let projection = DiffProjection::build_split(&lines, layout(side_text));
        (lines, projection)
    }

    /// Each display row as `old | new` text, with `~` for filler and the
    /// full text for single rows.
    fn cells(projection: &DiffProjection, lines: &[DiffLine]) -> Vec<String> {
        projection
            .rows(0, projection.len())
            .iter()
            .map(|row| match row {
                DisplayRow::Single(segment) => projection.row_text(lines, segment).to_owned(),
                DisplayRow::Split { old, new } => {
                    let text = |cell: &SideCell| {
                        cell.segment().map_or("~".to_owned(), |segment| {
                            projection.row_text(lines, segment).to_owned()
                        })
                    };
                    format!("{}|{}", text(old), text(new))
                }
            })
            .collect()
    }

    fn comment_rows(projection: &DiffProjection, side: DiffSide) -> Vec<Option<usize>> {
        projection
            .rows(0, projection.len())
            .iter()
            .map(|row| row.comment_row(side))
            .collect()
    }

    #[test]
    fn large_replacement_pairs_deletions_and_additions_side_by_side() {
        let (lines, projection) = split(
            "@@ -1,6 +1,6 @@\n keep\n-old one\n-old two\n-old three\n-old four\n+new one\n+new two\n+new three\n+new four\n tail",
            30,
        );

        assert_eq!(projection.mode(), DiffViewMode::Split);
        assert_eq!(
            cells(&projection, &lines),
            vec![
                "@@ -1,6 +1,6 @@",
                " keep| keep",
                "-old one|+new one",
                "-old two|+new two",
                "-old three|+new three",
                "-old four|+new four",
                " tail| tail",
            ]
        );
        // Paired deletion and addition stay separate canonical targets.
        assert_eq!(
            comment_rows(&projection, DiffSide::Old),
            vec![None, Some(1), Some(2), Some(3), Some(4), Some(5), Some(10)]
        );
        assert_eq!(
            comment_rows(&projection, DiffSide::New),
            vec![None, Some(1), Some(6), Some(7), Some(8), Some(9), Some(10)]
        );
        assert_eq!(projection.raw_row_side(2), Some(DiffSide::Old));
        assert_eq!(projection.raw_row_side(6), Some(DiffSide::New));
        assert_eq!(projection.raw_row_side(1), None);
        assert_eq!(projection.raw_row_rows(3), 3..4);
        assert_eq!(projection.raw_row_rows(7), 3..4);
        assert_eq!(projection.hunk_starts(), &[0]);
        assert_eq!(projection.gutter_width(), 4);
    }

    #[test]
    fn unequal_blocks_leave_filler_without_targets() {
        let (lines, projection) = split(
            "@@ -1,3 +1,1 @@\n-a\n-b\n-c\n+x\n@@ -9 +7,3 @@\n-d\n+y\n+z\n+w",
            30,
        );

        assert_eq!(
            cells(&projection, &lines),
            vec![
                "@@ -1,3 +1,1 @@",
                "-a|+x",
                "-b|~",
                "-c|~",
                "@@ -9 +7,3 @@",
                "-d|+y",
                "~|+z",
                "~|+w",
            ]
        );
        assert_eq!(
            comment_rows(&projection, DiffSide::New),
            vec![None, Some(4), None, None, None, Some(7), Some(8), Some(9)]
        );
        assert_eq!(
            comment_rows(&projection, DiffSide::Old),
            vec![None, Some(1), Some(2), Some(3), None, Some(6), None, None]
        );
    }

    #[test]
    fn pure_changes_have_an_empty_side_and_later_deletions_start_a_new_run() {
        let (lines, projection) = split(
            "@@ -1,3 +1,4 @@\n a\n+n1\n+n2\n b\n-o1\n c\n+n3\n-o2\n+n4",
            30,
        );

        assert_eq!(
            cells(&projection, &lines),
            vec![
                "@@ -1,3 +1,4 @@",
                " a| a",
                "~|+n1",
                "~|+n2",
                " b| b",
                "-o1|~",
                " c| c",
                // An addition followed by a deletion is not a replacement
                // run; the deletion pairs with the addition after it.
                "~|+n3",
                "-o2|+n4",
            ]
        );
    }

    #[test]
    fn alignment_never_crosses_hunk_boundaries_and_keeps_real_numbers() {
        let (lines, projection) = split(
            "@@ -1,2 +1,1 @@\n a\n-gone\n@@ -10,1 +9,2 @@\n+added\n b",
            30,
        );

        assert_eq!(
            cells(&projection, &lines),
            vec![
                "@@ -1,2 +1,1 @@",
                " a| a",
                "-gone|~",
                "@@ -10,1 +9,2 @@",
                "~|+added",
                " b| b",
            ]
        );
        assert_eq!(projection.hunk_starts(), &[0, 3]);
        let numbers: Vec<_> = lines
            .iter()
            .map(|line| (line.old_line, line.new_line))
            .collect();
        assert_eq!(numbers[1], (Some(1), Some(1)));
        assert_eq!(numbers[2], (Some(2), None));
        assert_eq!(numbers[4], (None, Some(9)));
        assert_eq!(numbers[5], (Some(10), Some(10)));
    }

    #[test]
    fn no_newline_markers_attach_to_their_side_without_breaking_pairing() {
        let marker = "\\ No newline at end of file";
        let (lines, projection) = split(
            &format!("@@ -1,2 +1,2 @@\n-old\n{marker}\n+new\n{marker}"),
            30,
        );
        assert_eq!(
            cells(&projection, &lines),
            vec![
                "@@ -1,2 +1,2 @@".to_owned(),
                "-old|+new".to_owned(),
                format!("{marker}|{marker}"),
            ]
        );
        assert_eq!(projection.raw_row_side(2), Some(DiffSide::Old));
        assert_eq!(projection.raw_row_side(4), Some(DiffSide::New));
        assert_eq!(projection.row(2).unwrap().comment_row(DiffSide::Old), None);
        assert_eq!(projection.row(2).unwrap().comment_row(DiffSide::New), None);

        // A marker only on the old side still lets the replacement pair up.
        let (lines, projection) = split(&format!("@@ -1 +1,2 @@\n-a\n{marker}\n+b\n+c"), 30);
        assert_eq!(
            cells(&projection, &lines),
            vec![
                "@@ -1 +1,2 @@".to_owned(),
                "-a|+b".to_owned(),
                // The marker belongs to the first pair's old side; the second
                // addition pairs with nothing.
                format!("{marker}|~"),
                "~|+c".to_owned(),
            ]
        );
        assert_eq!(projection.row(2).unwrap().comment_row(DiffSide::New), None);
        assert_eq!(
            projection.row(3).unwrap().comment_row(DiffSide::New),
            Some(4)
        );

        // Context markers show on both sides; a marker with nothing to attach
        // to spans the row.
        let (lines, projection) = split(&format!("@@ -1 +1 @@\n{marker}\n last\n{marker}"), 30);
        assert_eq!(
            cells(&projection, &lines),
            vec![
                "@@ -1 +1 @@".to_owned(),
                marker.to_owned(),
                " last| last".to_owned(),
                format!("{marker}|{marker}"),
            ]
        );
        assert!(matches!(projection.row(1), Some(DisplayRow::Single(_))));
        assert_eq!(projection.raw_row_side(3), None);
    }

    #[test]
    fn sides_wrap_independently_and_filler_below_a_shorter_side_has_no_target() {
        let long = "x".repeat(44);
        let (_, projection) = split(&format!("@@ -1 +1 @@\n-{long}\n+short"), 20);

        // 45 characters on the old side wrap to 20 + 20 + 5.
        assert_eq!(projection.len(), 4);
        assert_eq!(projection.raw_row_rows(1), 1..4);
        assert_eq!(projection.raw_row_rows(2), 1..2);
        assert_eq!(
            comment_rows(&projection, DiffSide::Old),
            vec![None, Some(1), Some(1), Some(1)]
        );
        assert_eq!(
            comment_rows(&projection, DiffSide::New),
            vec![None, Some(2), None, None]
        );
        let old: Vec<_> = projection
            .rows(1, 3)
            .iter()
            .map(|row| row.segment(DiffSide::Old).unwrap().char_offset)
            .collect();
        assert_eq!(old, vec![0, 20, 40]);
        // Filler reads the other side's line for anchoring only.
        assert_eq!(
            projection.anchor(3, DiffSide::New),
            Some(DiffAnchor {
                raw_row: 1,
                char_offset: 40
            })
        );
        assert_eq!(
            projection.row_for_anchor(DiffAnchor {
                raw_row: 1,
                char_offset: 25
            }),
            Some(2)
        );
    }

    #[test]
    fn unicode_and_wide_characters_wrap_per_side_like_rendering() {
        let old = "-日本語のテキストと説明の古い行です";
        let new = "+é zero\u{200b}width 👩‍💻 新しい行のテキストです done";
        let (lines, projection) = split(&format!("@@ -1 +1 @@\n{old}\n{new}"), 20);

        for (side, text, raw_row) in [(DiffSide::Old, old, 1), (DiffSide::New, new, 2)] {
            let shown: Vec<String> = projection
                .rows(1, projection.len())
                .iter()
                .filter_map(|row| row.segment(side))
                .map(|segment| {
                    assert_eq!(segment.raw_row, raw_row);
                    projection.row_text(&lines, segment).to_owned()
                })
                .collect();
            assert_eq!(shown, wrap_text(text, 20), "{side:?}");
        }
    }

    #[test]
    fn genuine_empty_lines_are_content_and_distinct_from_filler() {
        let (lines, projection) = split("@@ -1,2 +1,1 @@\n-\n-\n+", 30);

        assert_eq!(
            cells(&projection, &lines),
            vec!["@@ -1,2 +1,1 @@", "-|+", "-|~"]
        );
        let row = projection.row(1).unwrap();
        assert_eq!(row.comment_row(DiffSide::Old), Some(1));
        assert_eq!(row.comment_row(DiffSide::New), Some(3));
        assert!(matches!(
            projection.row(2),
            Some(DisplayRow::Split {
                new: SideCell::Filler,
                ..
            })
        ));
        assert_eq!(projection.row(2).unwrap().comment_row(DiffSide::New), None);
    }

    #[test]
    fn malformed_and_other_rows_span_both_sides_and_end_runs() {
        let (lines, projection) = split("not a hunk\n@@ broken\n@@ -1 +1 @@\n-a\nstray\n+b", 30);

        assert_eq!(
            cells(&projection, &lines),
            vec![
                "not a hunk",
                "@@ broken",
                "@@ -1 +1 @@",
                "-a|~",
                "stray",
                "~|+b"
            ]
        );
        assert_eq!(projection.hunk_count(), 1);
        for index in [0, 1, 2, 4] {
            let row = projection.row(index).unwrap();
            assert!(matches!(row, DisplayRow::Single(_)));
            assert_eq!(row.comment_row(DiffSide::Old), None);
            assert_eq!(row.comment_row(DiffSide::New), None);
        }
    }

    #[test]
    fn every_raw_row_round_trips_between_unified_and_split() {
        let patch = "@@ -1,4 +1,3 @@\n ctx\n-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\n-b\n+cc\n\\ No newline at end of file\n x";
        let lines = lines(patch);
        let unified = DiffProjection::build(&lines, 12, 7);
        let split = DiffProjection::build_split(&lines, layout(20));

        for raw_row in 0..lines.len() {
            for char_offset in [0, 5, 21, 99] {
                let anchor = DiffAnchor {
                    raw_row,
                    char_offset,
                };
                let row = split.row_for_anchor(anchor).unwrap();
                let side = split.raw_row_side(raw_row).unwrap_or(DiffSide::New);
                let segment = split.row(row).unwrap().segment(side).unwrap();
                assert_eq!(segment.raw_row, raw_row);
                assert!(segment.char_offset <= char_offset);
                let back = split.anchor(row, side).unwrap();
                let unified_row = unified.row_for_anchor(back).unwrap();
                assert_eq!(unified.row(unified_row).unwrap().primary().raw_row, raw_row);
            }
        }
    }

    #[test]
    fn split_layout_requires_twenty_text_columns_per_side() {
        let lines = lines("@@ -1 +1 @@\n-a\n+b");
        // One-digit numbers: gutter 4 per side, separator 1, so 49 columns
        // are needed for 20 text columns per side.
        assert_eq!(SplitLayout::fit(&lines, 48), None);
        let fit = SplitLayout::fit(&lines, 49).unwrap();
        assert_eq!((fit.side_gutter, fit.side_text), (4, 20));
        assert_eq!(fit.side_width() * 2 + SPLIT_SEPARATOR_WIDTH, 49);
        assert_eq!(SplitLayout::fit(&lines, 50).unwrap().side_text, 20);
        assert_eq!(SplitLayout::fit(&lines, 51).unwrap().side_text, 21);

        let wide = parse_patch_text("@@ -100 +1000 @@\n-a\n+b")
            .lines()
            .to_vec();
        // Four-digit numbers widen each gutter to 7.
        assert_eq!(SplitLayout::fit(&wide, 54), None);
        assert_eq!(SplitLayout::fit(&wide, 55).unwrap().side_gutter, 7);
    }
}
