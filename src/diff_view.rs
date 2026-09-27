//! Display-row projection of a parsed patch.
//!
//! The projection is the single index used by diff rendering and navigation.
//! Every display row is an address into the raw patch: the canonical raw row
//! (whose index is the GitHub line-comment position), the wrapped segment of
//! that row and the character offset at which the segment starts. It never
//! synthesizes content; rendering slices the original patch text.

use std::ops::Range;

use crate::inbox::{DiffLine, DiffLineKind};
use crate::ui_layout::for_each_wrap_segment;

/// How the diff pane lays out a patch. Part of the projection cache key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffViewMode {
    Unified,
}

/// A logical reading position that survives rebuilds at other widths.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiffAnchor {
    pub raw_row: usize,
    pub char_offset: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DisplayRow {
    /// Index of the patch row this display row shows.
    pub raw_row: usize,
    /// Wrapped segment index within the raw row; 0 for the first row.
    pub segment: usize,
    /// Character offset of the segment start within the raw row text.
    pub char_offset: usize,
    /// Byte range of the segment within the raw row text.
    pub bytes: Range<usize>,
    pub kind: DiffLineKind,
    /// Raw row a line comment would target, if the row kind can carry one.
    /// Continuations carry the original raw row; hunk headers, no-newline
    /// markers and unsupported rows carry none. Eligibility is still decided
    /// by `comment_draft::line_target`.
    pub comment_row: Option<usize>,
}

impl DisplayRow {
    pub fn is_continuation(&self) -> bool {
        self.segment > 0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffProjection {
    rows: Vec<DisplayRow>,
    /// First display row of each raw row, plus a final sentinel equal to the
    /// total row count.
    first_rows: Vec<usize>,
    hunk_starts: Vec<usize>,
    gutter_width: usize,
}

impl DiffProjection {
    /// Builds the projection in one pass over the patch. `content_width` is
    /// the number of terminal columns available to patch text.
    pub fn build(lines: &[DiffLine], content_width: usize, gutter_width: usize) -> Self {
        let mut rows = Vec::with_capacity(lines.len());
        let mut first_rows = Vec::with_capacity(lines.len() + 1);
        let mut hunk_starts = Vec::new();
        for (raw_row, line) in lines.iter().enumerate() {
            first_rows.push(rows.len());
            if line.kind == DiffLineKind::Hunk {
                hunk_starts.push(rows.len());
            }
            let comment_row = matches!(
                line.kind,
                DiffLineKind::Context | DiffLineKind::Addition | DiffLineKind::Deletion
            )
            .then_some(raw_row);
            let mut segment = 0;
            for_each_wrap_segment(&line.text, content_width, |wrapped| {
                rows.push(DisplayRow {
                    raw_row,
                    segment,
                    char_offset: wrapped.char_start,
                    bytes: wrapped.bytes,
                    kind: line.kind,
                    comment_row,
                });
                segment += 1;
            });
        }
        first_rows.push(rows.len());
        Self {
            rows,
            first_rows,
            hunk_starts,
            gutter_width,
        }
    }

    /// Gutter width for a patch: draft/comment marker plus old and new line
    /// numbers sized to the largest number present.
    pub fn gutter_width_for(lines: &[DiffLine]) -> usize {
        let max_number = lines
            .iter()
            .flat_map(|line| [line.old_line, line.new_line])
            .flatten()
            .max()
            .unwrap_or(0);
        let digits = max_number.to_string().len().max(1);
        digits.saturating_mul(2).saturating_add(5)
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

    /// Text shown by a display row, sliced from the original patch line.
    pub fn row_text<'a>(&self, lines: &'a [DiffLine], row: &DisplayRow) -> &'a str {
        lines
            .get(row.raw_row)
            .and_then(|line| line.text.get(row.bytes.clone()))
            .unwrap_or("")
    }

    /// Display rows occupied by a raw row.
    pub fn raw_row_rows(&self, raw_row: usize) -> Range<usize> {
        match self.first_rows.get(raw_row..=raw_row + 1) {
            Some([start, end]) => *start..*end,
            _ => 0..0,
        }
    }

    /// Display row that contains `anchor`. A raw row beyond the patch clamps
    /// to the last row; an offset beyond the text clamps to its last segment.
    pub fn row_for_anchor(&self, anchor: DiffAnchor) -> Option<usize> {
        let raw_count = self.first_rows.len().checked_sub(1)?;
        let raw_row = anchor.raw_row.min(raw_count.checked_sub(1)?);
        let range = self.raw_row_rows(raw_row);
        let segments = &self.rows[range.clone()];
        let within = segments
            .partition_point(|row| row.char_offset <= anchor.char_offset)
            .saturating_sub(1);
        Some(range.start + within)
    }

    pub fn anchor(&self, index: usize) -> Option<DiffAnchor> {
        self.rows.get(index).map(|row| DiffAnchor {
            raw_row: row.raw_row,
            char_offset: row.char_offset,
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
            .map(|row| projection.row_text(lines, row).to_owned())
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
            .map(|row| row.comment_row)
            .collect();
        assert_eq!(comment_rows, vec![None, Some(1), Some(2), Some(3), None]);
        assert_eq!(projection.row(4).unwrap().kind, DiffLineKind::NoNewline);
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
        let added: Vec<_> = projection.rows(1, 3).to_vec();
        assert!(added.iter().all(|row| row.raw_row == 1));
        assert!(added.iter().all(|row| row.comment_row == Some(1)));
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
        assert_eq!(projection.row(1).unwrap().comment_row, Some(1));
        assert_eq!(projection.row(2).unwrap().comment_row, Some(2));
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
                .all(|row| row.comment_row.is_none())
        );
    }

    #[test]
    fn anchors_round_trip_across_widths() {
        let lines = lines("@@ -1 +1 @@\n+0123456789abcdefghij\n z");
        let narrow = DiffProjection::build(&lines, 5, 7);
        let wide = DiffProjection::build(&lines, 8, 7);

        for index in 0..narrow.len() {
            let anchor = narrow.anchor(index).unwrap();
            assert_eq!(narrow.row_for_anchor(anchor), Some(index));
            let mapped = wide.row_for_anchor(anchor).unwrap();
            let row = wide.row(mapped).unwrap();
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
        let row = projection.row(index).unwrap();
        projection.row_text(lines, row).chars().count()
    }
}
