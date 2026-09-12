use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};

use crate::app::App;

const MIN_FULL_WIDTH: u16 = 60;
const MIN_FULL_HEIGHT: u16 = 16;

pub fn draw(frame: &mut Frame<'_>, app: &App) {
    let area = frame.area();
    if area.width < MIN_FULL_WIDTH || area.height < MIN_FULL_HEIGHT {
        draw_compact(frame, area);
    } else {
        draw_panes(frame, area, app);
    }
}

fn draw_panes(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(8), Constraint::Length(1)])
        .split(area);
    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(38), Constraint::Percentage(62)])
        .split(rows[0]);
    let left = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage(34),
            Constraint::Percentage(33),
            Constraint::Percentage(33),
        ])
        .split(columns[0]);

    draw_pane(frame, left[0], "Repository", app.fixture.repository, true);
    draw_pane(frame, left[1], "Commit", app.fixture.commit, false);
    draw_pane(frame, left[2], "File", app.fixture.file, false);

    let diff = app.fixture.diff_lines.join("\n");
    draw_pane(frame, columns[1], "Diff", &diff, false);

    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                " NORMAL ",
                Style::default().fg(Color::Black).bg(Color::Cyan),
            ),
            Span::raw(" fictional demo • q quit • navigation arrives in the next stage"),
        ])),
        rows[1],
    );
}

fn draw_pane(frame: &mut Frame<'_>, area: Rect, title: &str, content: &str, focused: bool) {
    let border_style = if focused {
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    let block = Block::default()
        .title(format!(" {title} "))
        .borders(Borders::ALL)
        .border_style(border_style);
    frame.render_widget(
        Paragraph::new(content)
            .block(block)
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn draw_compact(frame: &mut Frame<'_>, area: Rect) {
    let block = Block::default()
        .title(" ReviewBox ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));
    let message =
        Paragraph::new("Demo • terminal too small for panes\nResize to at least 60×16\nq quit")
            .block(block)
            .wrap(Wrap { trim: true });
    frame.render_widget(message, area);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixture::DemoFixture;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn rendered_text(width: u16, height: u16) -> String {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).expect("test terminal");
        let app = App::new(DemoFixture::load());
        terminal.draw(|frame| draw(frame, &app)).expect("draw");

        terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect()
    }

    #[test]
    fn full_layout_has_all_labeled_panes() {
        let output = rendered_text(100, 30);

        for label in ["Repository", "Commit", "File", "Diff"] {
            assert!(output.contains(label), "missing {label} pane");
        }
        assert!(output.contains("example-labs/orbit-notes"));
    }

    #[test]
    fn constrained_layout_uses_safe_fallback() {
        for (width, height) in [(1, 1), (10, 3), (59, 15)] {
            let _ = rendered_text(width, height);
        }

        assert!(rendered_text(40, 8).contains("terminal too small"));
    }
}
