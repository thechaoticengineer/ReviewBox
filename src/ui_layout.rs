use ratatui::layout::{Constraint, Direction, Layout, Rect};
use unicode_width::UnicodeWidthChar;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ReviewPaneLayout {
    pub(crate) repository: Rect,
    pub(crate) commit: Rect,
    pub(crate) file: Rect,
    pub(crate) diff: Rect,
    pub(crate) status: Rect,
}

pub(crate) fn wrap_text(text: &str, width: usize) -> Vec<String> {
    let mut rows = Vec::new();
    for_each_wrap_segment(text, width, |segment| {
        rows.push(text[segment.bytes].to_owned())
    });
    rows
}

/// One wrapped display row of a text: its byte range and the character offset
/// at which it starts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WrapSegment {
    pub(crate) bytes: std::ops::Range<usize>,
    pub(crate) char_start: usize,
}

/// Visits the wrapped rows of `text` at `width` terminal columns. This is the
/// single wrapping rule shared by rendering and diff navigation, so Unicode
/// widths always agree. Empty text still yields one empty row.
pub(crate) fn for_each_wrap_segment(text: &str, width: usize, mut visit: impl FnMut(WrapSegment)) {
    let width = width.max(1);
    let mut row_start = 0;
    let mut row_char_start = 0;
    let mut row_width: usize = 0;
    for (char_index, (byte_index, character)) in text.char_indices().enumerate() {
        let character_width = UnicodeWidthChar::width(character).unwrap_or(0);
        if row_width > 0 && row_width.saturating_add(character_width) > width {
            visit(WrapSegment {
                bytes: row_start..byte_index,
                char_start: row_char_start,
            });
            row_start = byte_index;
            row_char_start = char_index;
            row_width = 0;
        }
        row_width = row_width.saturating_add(character_width);
    }
    if row_start < text.len() || text.is_empty() {
        visit(WrapSegment {
            bytes: row_start..text.len(),
            char_start: row_char_start,
        });
    }
}

impl ReviewPaneLayout {
    pub(crate) fn from_area(area: Rect) -> Self {
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(8), Constraint::Length(1)])
            .split(area);
        let columns = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(44), Constraint::Percentage(56)])
            .split(rows[0]);
        let left = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Percentage(34),
                Constraint::Percentage(33),
                Constraint::Percentage(33),
            ])
            .split(columns[0]);

        Self {
            repository: left[0],
            commit: left[1],
            file: left[2],
            diff: columns[1],
            status: rows[1],
        }
    }

    pub(crate) fn content_heights(self) -> [usize; 4] {
        [
            self.repository.height.saturating_sub(2) as usize,
            self.commit.height.saturating_sub(2) as usize,
            self.file.height.saturating_sub(2) as usize,
            self.diff.height.saturating_sub(2) as usize,
        ]
    }

    pub(crate) fn diff_content_width(self) -> usize {
        self.diff.width.saturating_sub(2) as usize
    }
}

/// The expanded diff layout: a one-row identity header, the diff pane using
/// the rest of the area, and the status row. Replaces the three list panes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ExpandedDiffLayout {
    pub(crate) header: Rect,
    pub(crate) diff: Rect,
    pub(crate) status: Rect,
}

impl ExpandedDiffLayout {
    pub(crate) fn from_area(area: Rect) -> Self {
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Min(3),
                Constraint::Length(1),
            ])
            .split(area);
        Self {
            header: rows[0],
            diff: rows[1],
            status: rows[2],
        }
    }

    /// Diff pane content rows, excluding its top and bottom border. The
    /// header and status rows are already excluded by `from_area`.
    pub(crate) fn diff_content_height(self) -> usize {
        self.diff.height.saturating_sub(2) as usize
    }

    pub(crate) fn diff_content_width(self) -> usize {
        self.diff.width.saturating_sub(2) as usize
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pane_layout_stays_in_bounds_at_supported_and_boundary_sizes() {
        for (width, height) in [
            (120, 32),
            (80, 24),
            (60, 16),
            (59, 15),
            (40, 8),
            (20, 5),
            (1, 1),
            (0, 0),
        ] {
            let area = Rect::new(0, 0, width, height);
            let layout = ReviewPaneLayout::from_area(area);
            for pane in [
                layout.repository,
                layout.commit,
                layout.file,
                layout.diff,
                layout.status,
            ] {
                assert!(
                    pane.right() <= area.right(),
                    "width overflow at {width}x{height}"
                );
                assert!(
                    pane.bottom() <= area.bottom(),
                    "height overflow at {width}x{height}"
                );
            }
            assert_eq!(layout.repository.x, 0);
            assert_eq!(layout.diff.right(), area.right());
            assert_eq!(layout.status.bottom(), area.bottom());
        }
    }

    #[test]
    fn minimum_full_layout_preserves_content_in_every_pane_and_status() {
        let layout = ReviewPaneLayout::from_area(Rect::new(0, 0, 60, 16));

        assert!(
            layout
                .content_heights()
                .into_iter()
                .all(|height| height > 0)
        );
        assert!(layout.diff_content_width() > 0);
        assert_eq!(layout.status.height, 1);
    }

    #[test]
    fn expanded_layout_stays_in_bounds_and_has_a_one_row_header_and_status() {
        for (width, height) in [(120, 32), (80, 24), (60, 16)] {
            let area = Rect::new(0, 0, width, height);
            let layout = ExpandedDiffLayout::from_area(area);

            assert_eq!(layout.header.height, 1);
            assert_eq!(layout.status.height, 1);
            assert_eq!(layout.header.y, 0);
            assert_eq!(layout.diff.y, 1);
            assert_eq!(layout.status.bottom(), area.bottom());
            assert_eq!(layout.diff.right(), area.right());
            assert!(layout.diff_content_height() > 0, "at {width}x{height}");
            assert!(layout.diff_content_width() > 0, "at {width}x{height}");
            // The expanded diff pane gets far more width than the normal
            // 56% column, so the split threshold is easier to satisfy.
            let normal = ReviewPaneLayout::from_area(area);
            assert!(layout.diff_content_width() > normal.diff_content_width());
        }
    }
}
