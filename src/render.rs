use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};

use crate::app::{App, HELP_BINDINGS, MIN_FULL_HEIGHT, MIN_FULL_WIDTH, Mode, Pane};

pub fn draw(frame: &mut Frame<'_>, app: &App) {
    let area = frame.area();
    if area.width < MIN_FULL_WIDTH || area.height < MIN_FULL_HEIGHT {
        draw_compact(frame, area, app);
    } else {
        draw_panes(frame, area, app);
    }
    draw_modal(frame, area, app);
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

    draw_list_pane(
        frame,
        left[0],
        Pane::Repository,
        app,
        app.inbox()
            .repositories
            .iter()
            .map(|repository| repository.name.to_owned())
            .collect(),
        "repositories",
    );
    draw_list_pane(
        frame,
        left[1],
        Pane::Commit,
        app,
        app.current_commits()
            .iter()
            .map(|commit| commit.label())
            .collect(),
        "commits",
    );
    draw_list_pane(
        frame,
        left[2],
        Pane::File,
        app,
        app.current_files()
            .iter()
            .map(|file| file.path.to_owned())
            .collect(),
        "files",
    );
    draw_diff_pane(frame, columns[1], app);
    draw_status(frame, rows[1], app);
}

fn draw_list_pane(
    frame: &mut Frame<'_>,
    area: Rect,
    pane: Pane,
    app: &App,
    items: Vec<String>,
    empty_label: &str,
) {
    let selected = app.selected(pane);
    let scroll = app.scroll(pane);
    let lines = if items.is_empty() {
        vec![Line::styled(
            format!("  (no {empty_label})"),
            Style::default().fg(Color::DarkGray),
        )]
    } else {
        items
            .into_iter()
            .enumerate()
            .skip(scroll)
            .map(|(index, label)| {
                if index == selected {
                    Line::styled(
                        format!("> {label}"),
                        Style::default()
                            .fg(Color::Black)
                            .bg(Color::Cyan)
                            .add_modifier(Modifier::BOLD),
                    )
                } else {
                    Line::raw(format!("  {label}"))
                }
            })
            .collect()
    };

    frame.render_widget(
        Paragraph::new(lines).block(pane_block(pane, app.focus() == pane, app)),
        area,
    );
}

fn draw_diff_pane(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let lines = if app.current_diff_lines().is_empty() {
        let unavailable = !app.inbox().child_panes_available();
        vec![Line::styled(
            if unavailable {
                "  (file and diff content are unavailable for the live inbox)"
            } else {
                "  (no diff content in this fixture)"
            },
            Style::default().fg(Color::DarkGray),
        )]
    } else {
        app.current_diff_lines()
            .iter()
            .enumerate()
            .skip(app.scroll(Pane::Diff))
            .map(|(index, content)| {
                let color = if content.starts_with("@@") {
                    Color::Cyan
                } else if content.starts_with('+') {
                    Color::Green
                } else if content.starts_with('-') {
                    Color::Red
                } else {
                    Color::Reset
                };
                Line::from(vec![
                    Span::styled(
                        format!("{:>4} ", index + 1),
                        Style::default().fg(Color::DarkGray),
                    ),
                    Span::styled((*content).to_owned(), Style::default().fg(color)),
                ])
            })
            .collect()
    };

    frame.render_widget(
        Paragraph::new(lines).block(pane_block(Pane::Diff, app.focus() == Pane::Diff, app)),
        area,
    );
}

fn pane_block<'a>(pane: Pane, focused: bool, app: &App) -> Block<'a> {
    let border_style = if focused {
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    let (position, length) = app.position(pane);
    Block::default()
        .title(format!(" {} {position}/{length} ", pane.title()))
        .borders(Borders::ALL)
        .border_style(border_style)
}

fn draw_status(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let mode = match app.mode() {
        Mode::Normal if app.pending_g() => " NORMAL g ",
        Mode::Normal => " NORMAL ",
        Mode::SearchEntry { .. } => " SEARCH ",
        Mode::Help { .. } => " HELP ",
    };
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(mode, Style::default().fg(Color::Black).bg(Color::Cyan)),
            Span::raw(format!(
                " {} • {} • h/l focus • j/k move • gg/G ends • ^d/^u half • / search • ? help • q quit",
                app.focus().title(),
                app.status()
            )),
        ])),
        area,
    );
}

fn draw_compact(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let block = Block::default()
        .title(" ReviewBox ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));
    let mode = match app.mode() {
        Mode::Normal => "NORMAL",
        Mode::SearchEntry { .. } => "SEARCH",
        Mode::Help { .. } => "HELP",
    };
    let message = Paragraph::new(format!(
        "{mode} • {}\nterminal too small for panes\nresize to at least 60×16\n/ search • ? help • Esc back • q quit",
        app.focus().title()
    ))
    .block(block)
    .wrap(Wrap { trim: true });
    frame.render_widget(message, area);
}

fn draw_modal(frame: &mut Frame<'_>, area: Rect, app: &App) {
    match app.mode() {
        Mode::Normal => {}
        Mode::SearchEntry { target } => draw_search(frame, area, app, target),
        Mode::Help { .. } => draw_help(frame, area),
    }
}

fn draw_search(frame: &mut Frame<'_>, area: Rect, app: &App, target: Pane) {
    let popup = centered_rect(area, 64, 5);
    frame.render_widget(Clear, popup);
    let content = format!(
        "/{}\nEnter: search {} • Esc: cancel • Backspace: edit",
        app.search_query(),
        target.title()
    );
    frame.render_widget(
        Paragraph::new(content)
            .block(modal_block(format!(" Search {} ", target.title())))
            .wrap(Wrap { trim: false }),
        popup,
    );
}

fn draw_help(frame: &mut Frame<'_>, area: Rect) {
    let desired_height = u16::try_from(HELP_BINDINGS.len())
        .unwrap_or(u16::MAX)
        .saturating_add(4);
    let popup = centered_rect(area, 72, desired_height);
    frame.render_widget(Clear, popup);

    let mut lines = Vec::with_capacity(HELP_BINDINGS.len() + 2);
    lines.push(Line::styled(
        "Normal mode",
        Style::default().add_modifier(Modifier::BOLD),
    ));
    lines.extend(HELP_BINDINGS.iter().map(|binding| {
        Line::from(vec![
            Span::styled(
                format!("{:<24}", binding.keys),
                Style::default().fg(Color::Cyan),
            ),
            Span::raw(binding.action),
        ])
    }));
    lines.push(Line::raw("Press Escape to close"));

    frame.render_widget(
        Paragraph::new(lines)
            .block(modal_block(" Keyboard help "))
            .wrap(Wrap { trim: true }),
        popup,
    );
}

fn modal_block<'a>(title: impl Into<Line<'a>>) -> Block<'a> {
    Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan))
}

fn centered_rect(area: Rect, desired_width: u16, desired_height: u16) -> Rect {
    let width = desired_width.min(area.width);
    let height = desired_height.min(area.height);
    Rect::new(
        area.x.saturating_add(area.width.saturating_sub(width) / 2),
        area.y
            .saturating_add(area.height.saturating_sub(height) / 2),
        width,
        height,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{Command, Input};
    use crate::fixture::DemoFixture;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn rendered_text(app: &mut App, width: u16, height: u16) -> String {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).expect("test terminal");
        app.resize(width, height);
        terminal.draw(|frame| draw(frame, app)).expect("draw");

        terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect()
    }

    #[test]
    fn full_layout_has_populated_labeled_panes_and_status() {
        let mut app = App::new(DemoFixture::load());
        let output = rendered_text(&mut app, 120, 32);

        for label in ["Repository", "Commit", "File", "Diff", "NORMAL"] {
            assert!(output.contains(label), "missing {label}");
        }
        assert!(output.contains("fictional-labs/orbit-notes-demo"));
        assert!(output.contains("a1b2c3d"));
        assert!(output.contains("src/welcome.rs"));
        assert!(output.contains("Welcome aboard"));
    }

    #[test]
    fn parent_selection_updates_visible_child_context() {
        let mut app = App::new(DemoFixture::load());
        app.apply(Command::MoveDown);
        let output = rendered_text(&mut app, 120, 32);

        assert!(output.contains("fictional-studio/pixel-garden-demo"));
        assert!(output.contains("13579bd"));
        assert!(output.contains("src/lib.rs"));
    }

    #[test]
    fn pending_prefix_is_visible() {
        let mut app = App::new(DemoFixture::load());
        app.apply(Command::GPrefix);

        assert!(rendered_text(&mut app, 120, 32).contains("NORMAL g"));
    }

    #[test]
    fn search_overlay_shows_captured_target_query_and_controls() {
        let mut app = App::new(DemoFixture::load());
        app.handle_input(Input::Character('l'));
        app.handle_input(Input::Character('/'));
        for character in "route".chars() {
            app.handle_input(Input::Character(character));
        }

        let output = rendered_text(&mut app, 120, 32);

        assert!(output.contains("Search Commit"));
        assert!(output.contains("/route"));
        assert!(output.contains("Backspace: edit"));
    }

    #[test]
    fn help_overlay_is_derived_from_the_shared_binding_table() {
        let mut app = App::new(DemoFixture::load());
        app.handle_input(Input::Character('?'));

        let output = rendered_text(&mut app, 120, 32);

        assert!(output.contains("Keyboard help"));
        for binding in HELP_BINDINGS {
            assert!(
                output.contains(binding.keys),
                "help is missing {}",
                binding.keys
            );
            assert!(
                output.contains(binding.action),
                "help is missing {}",
                binding.action
            );
        }
    }

    #[test]
    fn constrained_layout_uses_safe_fallback() {
        for (width, height) in [(1, 1), (10, 3), (59, 15)] {
            let mut app = App::new(DemoFixture::load());
            let _ = rendered_text(&mut app, width, height);
        }

        let mut app = App::new(DemoFixture::load());
        assert!(rendered_text(&mut app, 40, 8).contains("terminal too small"));
    }

    #[test]
    fn modal_overlays_are_safe_at_constrained_and_zero_like_dimensions() {
        for open in [Input::Character('/'), Input::Character('?')] {
            for (width, height) in [(1, 1), (2, 2), (10, 3), (40, 8), (59, 15)] {
                let mut app = App::new(DemoFixture::load());
                app.handle_input(open);
                let _ = rendered_text(&mut app, width, height);
            }
        }

        assert_eq!(centered_rect(Rect::new(10, 20, 0, 0), 72, 15).width, 0);
    }
}
