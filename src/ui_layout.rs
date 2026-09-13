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
    let width = width.max(1);
    let mut rows = Vec::new();
    let mut row = String::new();
    let mut row_width: usize = 0;
    for character in text.chars() {
        let character_width = UnicodeWidthChar::width(character).unwrap_or(0);
        if row_width > 0 && row_width.saturating_add(character_width) > width {
            rows.push(std::mem::take(&mut row));
            row_width = 0;
        }
        row.push(character);
        row_width = row_width.saturating_add(character_width);
    }
    if row.is_empty() && rows.is_empty() {
        rows.push(String::new());
    } else if !row.is_empty() {
        rows.push(row);
    }
    rows
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
