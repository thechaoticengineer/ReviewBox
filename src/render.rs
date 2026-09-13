use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};

use crate::app::{App, HELP_BINDINGS, LivePhase, MIN_FULL_HEIGHT, MIN_FULL_WIDTH, Mode, Pane};
use crate::day::TimezoneSource;
use crate::github::{DetailFailure, DetailState, FailureCategory, RESPONSE_TRUNCATED_LABEL};
use crate::inbox::{DiffLineKind, InboxSource, PatchContent};
use crate::ui_layout::{ReviewPaneLayout, wrap_text};
use unicode_width::UnicodeWidthChar;

pub(crate) const NO_PATCH_LABEL: &str =
    "No GitHub text patch (binary, rename/mode-only, or empty file)";

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
    let layout = ReviewPaneLayout::from_area(area);
    draw_review_panes(frame, layout, app);
    draw_status(frame, layout.status, app);
}

fn draw_review_panes(frame: &mut Frame<'_>, layout: ReviewPaneLayout, app: &App) {
    draw_list_pane(
        frame,
        layout.repository,
        Pane::Repository,
        app,
        app.visible_repositories()
            .iter()
            .map(|repository| sanitize_display_text(&repository.display_name()))
            .collect(),
        if app.all_reviewed_empty() {
            "remaining commits — ALL REVIEWED"
        } else {
            "repositories"
        },
    );
    draw_list_pane(
        frame,
        layout.commit,
        Pane::Commit,
        app,
        app.current_commits()
            .iter()
            .map(|commit| {
                let (reviewed, drafted) =
                    app.current_repository()
                        .map_or((false, false), |repository| {
                            (
                                app.is_reviewed(repository.identity.id, &commit.sha),
                                app.commit_has_draft(repository.identity.id, &commit.sha),
                            )
                        });
                let review_marker = if reviewed { "✓" } else { " " };
                let draft_marker = if drafted { "◆" } else { " " };
                format!(
                    "{review_marker}{draft_marker} {}",
                    sanitize_display_text(&commit.label())
                )
            })
            .collect(),
        "commits",
    );
    draw_list_pane(
        frame,
        layout.file,
        Pane::File,
        app,
        app.current_files()
            .iter()
            .map(|file| sanitize_display_text(&file.path))
            .collect(),
        "files",
    );
    draw_diff_pane(frame, layout.diff, app);
}

fn draw_live_summary(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let InboxSource::Live { selection } = &app.inbox().source else {
        return;
    };
    let Some(state) = app.live_state() else {
        return;
    };

    let fallback = if selection.timezone_source == TimezoneSource::Fallback {
        " (local detection failed; UTC fallback)"
    } else {
        ""
    };
    let phase = match state.phase {
        LivePhase::Authenticating => "Loading: checking gh authentication".to_owned(),
        LivePhase::Discovering {
            page,
            owned_repositories,
        } => format!("Discovering repositories: page {page}, {owned_repositories} owned found"),
        LivePhase::LoadingRepository { repository, total } => {
            format!("Loading repository {repository}/{total}")
        }
        LivePhase::LoadingBranches {
            repository,
            page,
            branches,
        } => {
            format!("Discovering branches: repository {repository}, page {page}, {branches} found")
        }
        LivePhase::LoadingCommits {
            repository,
            branch,
            branches,
            page,
            accepted_commits,
        } => format!(
            "Loading commits: repository {repository}, branch {branch}/{branches}, page {}, {} accepted",
            page.max(1),
            accepted_commits
        ),
        LivePhase::Complete => "COMPLETE — daily inbox loaded".to_owned(),
        LivePhase::EmptyDay => "EMPTY DAY — no matching commits".to_owned(),
        LivePhase::NoOwnedRepositories => {
            "NO OWNED REPOSITORIES — nothing available to scan".to_owned()
        }
        LivePhase::Incomplete => "INCOMPLETE RESULTS — partial data remains usable".to_owned(),
        LivePhase::Fatal => format!(
            "LOAD FAILED — {}",
            state
                .failures()
                .first()
                .map(|failure| failure_category_label(failure.category))
                .unwrap_or("unknown GitHub failure")
        ),
    };

    let progress = &state.progress;
    let mut lines = vec![
        Line::styled(
            format!("Selected day: {}", selection.date),
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Line::raw(format!("Timezone: {}{fallback}", selection.timezone_name)),
        Line::raw(""),
        Line::styled(phase, Style::default().fg(phase_color(&state.phase))),
        Line::raw(format!(
            "Repositories: {}/{} processed",
            progress.repositories_processed, progress.repositories_discovered
        )),
        Line::raw(format!(
            "Branches: {}/{} processed",
            progress.branches_processed, progress.branches_discovered
        )),
        Line::raw(format!("Commits available: {}", state.available_commits())),
    ];

    if state.incomplete_repositories() > 0 {
        lines.push(Line::styled(
            format!(
                "Warning: {} repositories have incomplete coverage",
                state.incomplete_repositories()
            ),
            Style::default().fg(Color::Yellow),
        ));
    }
    if let Some(warning) = app.review_warning() {
        lines.push(Line::styled(
            format!("Review progress warning: {warning}"),
            Style::default().fg(Color::Yellow),
        ));
    }
    if let Some(warning) = app.draft_warning() {
        lines.push(Line::styled(
            format!("Comment draft warning: {warning}"),
            Style::default().fg(Color::Yellow),
        ));
    }
    lines.push(Line::raw(if app.remaining_only() {
        "Inbox filter: remaining commits only"
    } else {
        "Inbox filter: all commits"
    }));
    if !state.failures().is_empty() {
        lines.push(Line::raw(""));
        lines.push(Line::styled(
            "Bounded error summary:",
            Style::default().add_modifier(Modifier::BOLD),
        ));
        lines.extend(state.failures().iter().map(|failure| {
            Line::styled(
                format!("• {failure}"),
                Style::default().fg(failure_color(failure.category)),
            )
        }));
        if state.omitted_failures() > 0 {
            lines.push(Line::raw(format!(
                "• {} additional errors omitted",
                state.omitted_failures()
            )));
        }
    }
    frame.render_widget(
        Paragraph::new(lines)
            .block(
                pane_block(Pane::Diff, app.focus() == Pane::Diff, app)
                    .title(" Diff — GitHub daily inbox "),
            )
            .wrap(Wrap { trim: true }),
        area,
    );
}

fn phase_color(phase: &LivePhase) -> Color {
    match phase {
        LivePhase::Complete | LivePhase::EmptyDay | LivePhase::NoOwnedRepositories => Color::Green,
        LivePhase::Incomplete => Color::Yellow,
        LivePhase::Fatal => Color::Red,
        _ => Color::Cyan,
    }
}

fn failure_color(category: FailureCategory) -> Color {
    match category {
        FailureCategory::RateLimit | FailureCategory::PermissionOrNotFound => Color::Yellow,
        _ => Color::Red,
    }
}

fn failure_category_label(category: FailureCategory) -> &'static str {
    match category {
        FailureCategory::Authentication => "authentication failed",
        FailureCategory::MissingGh => "GitHub CLI not found",
        FailureCategory::PermissionOrNotFound => "permission denied or resource unavailable",
        FailureCategory::RateLimit => "GitHub API rate limit reached",
        FailureCategory::MalformedResponse => "unreadable GitHub response",
        FailureCategory::MalformedJson => "malformed GitHub JSON",
        FailureCategory::Offline => "offline",
        FailureCategory::Transport => "GitHub transport failure",
        FailureCategory::Command => "GitHub CLI request failure",
        FailureCategory::Api => "GitHub API failure",
        FailureCategory::Cancelled => "loading cancelled",
    }
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
    if matches!(app.inbox().source, InboxSource::Live { .. })
        && app.current_detail_state().is_none()
    {
        draw_live_summary(frame, area, app);
        return;
    }

    let lines = if matches!(app.current_detail_state(), Some(DetailState::Loading)) {
        diff_message(
            app,
            area,
            "Loading commit details…",
            Style::default().fg(Color::Cyan),
        )
    } else if let Some(DetailState::Failed(failure)) = app.current_detail_state() {
        let label = match failure {
            DetailFailure::ResponseTruncated => RESPONSE_TRUNCATED_LABEL.to_owned(),
            DetailFailure::Load(failure) => format!(
                "Commit details unavailable: {}",
                failure_category_label(failure.category)
            ),
        };
        diff_message(app, area, &label, Style::default().fg(Color::Red))
    } else if matches!(app.inbox().source, InboxSource::Demo)
        && matches!(
            app.current_commit().map(|commit| &commit.files),
            Some(crate::inbox::ChildPane::ResponseTruncated)
        )
    {
        diff_message(
            app,
            area,
            RESPONSE_TRUNCATED_LABEL,
            Style::default().fg(Color::Red),
        )
    } else if matches!(app.inbox().source, InboxSource::Demo)
        && matches!(
            app.current_commit().map(|commit| &commit.files),
            Some(crate::inbox::ChildPane::Unavailable)
        )
    {
        diff_message(
            app,
            area,
            "Commit file list unavailable",
            Style::default().fg(Color::Yellow),
        )
    } else if let Some(file) = app.current_file() {
        match &file.patch {
            PatchContent::Empty => diff_message(
                app,
                area,
                "No textual changes",
                Style::default().fg(Color::DarkGray),
            ),
            PatchContent::NoPatch => diff_message(
                app,
                area,
                NO_PATCH_LABEL,
                Style::default().fg(Color::Yellow),
            ),
            PatchContent::Unavailable => diff_message(
                app,
                area,
                "Patch not provided by GitHub (binary or too large)",
                Style::default().fg(Color::Yellow),
            ),
            PatchContent::Capped {
                reason: crate::inbox::PatchCapReason::CommitBudget,
                ..
            } => diff_message(
                app,
                area,
                "Omitted by per-commit budget",
                Style::default().fg(Color::Yellow),
            ),
            PatchContent::Text { .. } | PatchContent::Capped { .. } => {
                diff_lines(app, area.height.saturating_sub(2) as usize)
            }
        }
    } else if app.current_diff_lines().is_empty() {
        diff_message(
            app,
            area,
            "No textual changes",
            Style::default().fg(Color::DarkGray),
        )
    } else {
        diff_lines(app, area.height.saturating_sub(2) as usize)
    };

    frame.render_widget(
        Paragraph::new(lines).block(pane_block(Pane::Diff, app.focus() == Pane::Diff, app)),
        area,
    );
}

fn diff_message(app: &App, area: Rect, message: &str, style: Style) -> Vec<Line<'static>> {
    let mut lines = diff_notice(app);
    lines.extend(
        wrap_text(message, area.width.saturating_sub(2) as usize)
            .into_iter()
            .map(|row| Line::styled(row, style)),
    );
    lines
}

fn diff_lines(app: &App, available_rows: usize) -> Vec<Line<'static>> {
    let mut lines = diff_notice(app);
    let gutter_width = app.diff_gutter_width();
    let number_width = gutter_width.saturating_sub(5) / 2;
    let content_width = app.diff_content_width();
    for (index, content) in app
        .current_diff_lines()
        .iter()
        .enumerate()
        .skip(app.scroll(Pane::Diff))
    {
        let style = diff_style(content.kind);
        let matched = app.diff_match() == Some(index);
        let selected = app.diff_cursor() == index;
        let drafted = app.diff_line_has_draft(index);
        for (row, text) in wrap_text(&content.text, content_width)
            .into_iter()
            .enumerate()
            .skip(if index == app.scroll(Pane::Diff) {
                app.diff_row_offset()
            } else {
                0
            })
        {
            if lines.len() >= available_rows {
                return lines;
            }
            let gutter = if row == 0 {
                format_diff_gutter(drafted, content.old_line, content.new_line, number_width)
            } else {
                " ".repeat(gutter_width)
            };
            let text_style = if selected {
                style.add_modifier(Modifier::REVERSED)
            } else if matched {
                style.add_modifier(Modifier::BOLD)
            } else {
                style
            };
            lines.push(Line::from(vec![
                Span::styled(
                    gutter,
                    if selected {
                        Style::default()
                            .fg(Color::Black)
                            .bg(Color::Cyan)
                            .add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(Color::DarkGray)
                    },
                ),
                Span::styled(text, text_style),
            ]));
        }
    }
    lines
}

fn diff_notice(app: &App) -> Vec<Line<'static>> {
    app.diff_notice_texts()
        .iter()
        .flat_map(|notice| {
            wrap_text(notice, app.diff_viewport_width())
                .into_iter()
                .map(|row| Line::styled(row, Style::default().fg(Color::Yellow)))
        })
        .collect()
}

fn diff_style(kind: DiffLineKind) -> Style {
    match kind {
        DiffLineKind::Hunk => Style::default().fg(Color::Cyan),
        DiffLineKind::Addition => Style::default().fg(Color::Green),
        DiffLineKind::Deletion => Style::default().fg(Color::Red),
        DiffLineKind::NoNewline | DiffLineKind::Other => Style::default().fg(Color::DarkGray),
        DiffLineKind::Context => Style::default(),
    }
}

fn format_diff_gutter(drafted: bool, old: Option<u32>, new: Option<u32>, width: usize) -> String {
    let old = old.map_or_else(String::new, |number| number.to_string());
    let new = new.map_or_else(String::new, |number| number.to_string());
    let marker = if drafted { '◆' } else { ' ' };
    format!("{marker} {old:>width$} {new:>width$} ")
}

fn sanitize_display_text(value: &str) -> String {
    let value = value.strip_suffix('\r').unwrap_or(value);
    let mut sanitized = String::with_capacity(value.len());
    for character in value.chars() {
        if character == '\t' {
            sanitized.push_str("    ");
        } else if character.is_control() {
            sanitized.push('\u{fffd}');
        } else {
            sanitized.push(character);
        }
    }
    sanitized
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
    let (reviewed, total) = app.review_progress();
    let filter = if app.remaining_only() {
        "remaining"
    } else {
        "all"
    };
    let title = if pane == Pane::Diff {
        format!(
            " {} • {} • {filter} {reviewed}/{total} reviewed • {position}/{length} ",
            pane.title(),
            app.diff_comment_feedback()
        )
    } else {
        format!(
            " {} {filter} {reviewed}/{total} reviewed • {position}/{length} ",
            pane.title()
        )
    };
    Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(border_style)
}

fn draw_status(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let mode = match app.mode() {
        Mode::Normal if app.pending_g() => " NORMAL g ",
        Mode::Normal => " NORMAL ",
        Mode::SearchEntry { .. } => " SEARCH ",
        Mode::Help { .. } => " HELP ",
        Mode::Edit { .. } => " EDIT ",
    };
    let mut spans = vec![
        Span::styled(mode, Style::default().fg(Color::Black).bg(Color::Cyan)),
        Span::raw(format!(" {} • {}", app.focus().title(), app.status())),
    ];
    if let Some(warning) = app.review_warning() {
        spans.push(Span::styled(
            format!(" • REVIEW STATE WARNING: {warning}"),
            Style::default().fg(Color::Yellow),
        ));
    }
    if let Some(warning) = app.draft_warning() {
        spans.push(Span::styled(
            format!(" • COMMENT DRAFT WARNING: {warning}"),
            Style::default().fg(Color::Yellow),
        ));
    }
    if app.draft_store_available() && app.draft_count() > 0 {
        spans.push(Span::raw(format!(" • {} drafts", app.draft_count())));
    }
    spans.push(Span::raw(
        " • c comment • h/l focus • j/k move • m review • f filter • / search • n/N match • ? help • q quit",
    ));
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
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
        Mode::Edit { .. } => "EDIT",
    };
    let review_warning = app
        .review_warning()
        .map(|warning| format!("\nREVIEW STATE WARNING: {warning}"))
        .unwrap_or_default();
    let draft_warning = app
        .draft_warning()
        .map(|warning| format!("\nCOMMENT DRAFT WARNING: {warning}"))
        .unwrap_or_default();
    let (reviewed, total) = app.review_progress();
    let message = Paragraph::new(format!(
        "{mode} • {} • {} {reviewed}/{total} reviewed\nterminal too small for panes\nresize to at least 60×16\nc comment • m review • f filter • / search • n/N match • ? help • q quit{review_warning}{draft_warning}",
        app.focus().title(),
        if app.remaining_only() { "remaining" } else { "all" },
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
        Mode::Edit { target, .. } => draw_edit(frame, area, app, &target),
    }
}

fn draw_edit(
    frame: &mut Frame<'_>,
    area: Rect,
    app: &App,
    target: &crate::comment_draft::CommentTarget,
) {
    let popup = if area.width < MIN_FULL_WIDTH || area.height < MIN_FULL_HEIGHT {
        area
    } else {
        centered_rect(area, 82, 18)
    };
    frame.render_widget(Clear, popup);
    let block = modal_block(" EDIT comment ");
    let inner = block.inner(popup);
    frame.render_widget(block, popup);
    let Some(buffer) = app.edit_buffer() else {
        return;
    };
    if inner.width == 0 || inner.height == 0 {
        return;
    }

    let header_rows = inner.height.min(2);
    let footer_rows = u16::from(inner.height > header_rows);
    let body_height = inner.height.saturating_sub(header_rows + footer_rows);
    let header = Rect::new(inner.x, inner.y, inner.width, header_rows);
    frame.render_widget(
        Paragraph::new(vec![
            Line::raw(format!("Target: {}", app.edit_target_description(target))),
            Line::styled(
                format!(
                    "{} • {}",
                    if buffer.is_saved() {
                        "Saved draft"
                    } else {
                        "New draft"
                    },
                    app.status()
                ),
                if app.status().contains("not saved") {
                    Style::default().fg(Color::Red)
                } else {
                    Style::default().fg(Color::Yellow)
                },
            ),
        ]),
        header,
    );

    if body_height > 0 {
        let body = Rect::new(
            inner.x,
            inner.y.saturating_add(header_rows),
            inner.width,
            body_height,
        );
        let (rows, cursor_row, cursor_column) =
            wrapped_edit_rows(buffer.text(), buffer.cursor(), body.width as usize);
        let capacity = body.height as usize;
        let scroll = cursor_row.saturating_add(1).saturating_sub(capacity);
        let visible = rows
            .into_iter()
            .skip(scroll)
            .take(capacity)
            .map(Line::raw)
            .collect::<Vec<_>>();
        frame.render_widget(Paragraph::new(visible), body);
        let cursor_y = body
            .y
            .saturating_add(u16::try_from(cursor_row.saturating_sub(scroll)).unwrap_or(u16::MAX));
        let cursor_x = body.x.saturating_add(
            u16::try_from(cursor_column.min(body.width.saturating_sub(1) as usize))
                .unwrap_or(u16::MAX),
        );
        if cursor_x < body.right() && cursor_y < body.bottom() {
            frame.set_cursor_position((cursor_x, cursor_y));
        }
    }

    if footer_rows > 0 {
        let footer = Rect::new(inner.x, inner.bottom().saturating_sub(1), inner.width, 1);
        frame.render_widget(
            Paragraph::new(
                "Esc save/return • Ctrl-g cancel • Ctrl-c save/quit • arrows/Home/End move",
            )
            .style(Style::default().fg(Color::Cyan)),
            footer,
        );
    }
}

fn wrapped_edit_rows(text: &str, cursor: usize, width: usize) -> (Vec<String>, usize, usize) {
    let width = width.max(1);
    let mut rows = vec![String::new()];
    let mut row = 0;
    let mut column: usize = 0;
    let mut cursor_position = None;
    for (index, character) in text.char_indices() {
        if character != '\n' {
            let character_width = match character {
                '\t' => 4,
                character if character.is_control() => 1,
                character => UnicodeWidthChar::width(character).unwrap_or(0),
            };
            if column > 0 && column.saturating_add(character_width) > width {
                rows.push(String::new());
                row += 1;
                column = 0;
            }
        }
        if index == cursor {
            cursor_position = Some((row, column));
        }
        if character == '\n' {
            rows.push(String::new());
            row += 1;
            column = 0;
        } else {
            let (display, character_width) = match character {
                '\t' => ("    ".to_owned(), 4),
                character if character.is_control() => ("�".to_owned(), 1),
                character => (
                    character.to_string(),
                    UnicodeWidthChar::width(character).unwrap_or(0),
                ),
            };
            rows[row].push_str(&display);
            column = column.saturating_add(character_width);
        }
    }
    if cursor == text.len() {
        if column >= width {
            rows.push(String::new());
            row += 1;
            column = 0;
        }
        cursor_position = Some((row, column));
    }
    let (cursor_row, cursor_column) = cursor_position.unwrap_or((0, 0));
    (rows, cursor_row, cursor_column)
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
    use crate::app::{Command, DetailEffect, DetailResult, Input};
    use crate::fixture::DemoFixture;
    use crate::github::{
        FailureScope, LoadEvent, LoadFailure, LoadProgress, LoadStatus, LoadedRepository,
        RepositoryCoverage,
    };
    use crate::inbox::{
        ChildPane, Commit, CommitDetail, DiffLine, DiffLineKind, FileChange, FileStatus,
        GitHubAuthor, Inbox, PatchCapReason, PatchContent, Repository, RepositoryIdentity,
    };
    use crate::review_state::ReviewStateError;
    use chrono::{TimeZone, Utc};
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

    fn rendered_diff_prefix(app: &mut App, width: u16, height: u16, rows: usize) -> String {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).expect("test terminal");
        app.resize(width, height);
        terminal.draw(|frame| draw(frame, app)).expect("draw");

        let diff = ReviewPaneLayout::from_area(Rect::new(0, 0, width, height)).diff;
        let buffer = terminal.backend().buffer();
        let mut text = String::new();
        for y in diff.y.saturating_add(1)
            ..diff
                .y
                .saturating_add(1)
                .saturating_add(rows as u16)
                .min(diff.bottom().saturating_sub(1))
        {
            for x in diff.x.saturating_add(1)..diff.right().saturating_sub(1) {
                text.push_str(buffer[(x, y)].symbol());
            }
        }
        text
    }

    fn live_app() -> App {
        let selection = crate::day::select_day(
            crate::day::parse_date("2024-01-15").unwrap(),
            crate::day::parse_timezone("Europe/Warsaw").unwrap(),
            TimezoneSource::Explicit,
        )
        .unwrap();
        App::new(Inbox::live(selection))
    }

    fn fatal_app(category: FailureCategory) -> App {
        let mut app = live_app();
        app.apply_load_event(LoadEvent::Failure(LoadFailure {
            category,
            scope: FailureScope::Authentication,
            http_status: None,
        }));
        app.apply_load_event(LoadEvent::Finished {
            status: LoadStatus::Fatal,
            progress: LoadProgress::default(),
        });
        app
    }

    fn live_commit_app() -> App {
        let mut app = live_app();
        app.apply_load_event(LoadEvent::RepositorySnapshot {
            repository_index: 0,
            repository: LoadedRepository {
                repository: Repository {
                    identity: RepositoryIdentity {
                        id: 88,
                        owner: "fixture".to_owned(),
                        name: "details".to_owned(),
                    },
                    commits: vec![Commit {
                        sha: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_owned(),
                        subject: "render live detail".to_owned(),
                        author: GitHubAuthor {
                            login: "fixture-user".to_owned(),
                        },
                        authored_at: Utc.with_ymd_and_hms(2024, 1, 15, 12, 0, 0).unwrap(),
                        files: ChildPane::Unavailable,
                    }],
                },
                branch_count: 1,
                coverage: RepositoryCoverage::Complete,
            },
        });
        app
    }

    fn request_identity(app: &mut App) -> (u64, crate::app::DetailKey) {
        let effects = app.take_detail_effects();
        let [
            DetailEffect::Request {
                request_id, key, ..
            },
        ] = effects.as_slice()
        else {
            panic!("expected detail request");
        };
        (*request_id, key.clone())
    }

    fn demo_patch_app(patch: PatchContent) -> App {
        App::new(Inbox::demo(vec![Repository {
            identity: RepositoryIdentity {
                id: 91,
                owner: "fixture".to_owned(),
                name: "patches".to_owned(),
            },
            commits: vec![Commit {
                sha: "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".to_owned(),
                subject: "render patch states".to_owned(),
                author: GitHubAuthor {
                    login: "fixture-user".to_owned(),
                },
                authored_at: Utc.with_ymd_and_hms(2024, 1, 15, 12, 0, 0).unwrap(),
                files: ChildPane::Available(vec![FileChange {
                    path: "src/patch.rs".to_owned(),
                    api_path_is_commentable: true,
                    previous_path: None,
                    status: FileStatus::Modified,
                    additions: 1,
                    deletions: 1,
                    changes: 2,
                    patch,
                }]),
            }],
        }]))
    }

    fn notice_patch_lines() -> Vec<DiffLine> {
        (1..=13)
            .map(|number| DiffLine {
                kind: DiffLineKind::Addition,
                old_line: None,
                new_line: Some(number),
                text: format!(
                    "+LINE{number:02}{}",
                    if number == 1 || number == 13 {
                        " MATCH"
                    } else {
                        ""
                    }
                ),
            })
            .collect()
    }

    fn live_file_notice_app(patch: PatchContent) -> App {
        let mut app = live_commit_app();
        app.apply(Command::Open);
        app.apply(Command::Open);
        let (request_id, key) = request_identity(&mut app);
        app.apply_detail_result(DetailResult {
            request_id,
            key,
            outcome: Ok(CommitDetail {
                files: vec![FileChange {
                    path: "src/notice.rs".to_owned(),
                    api_path_is_commentable: true,
                    previous_path: None,
                    status: FileStatus::Modified,
                    additions: 13,
                    deletions: 0,
                    changes: 13,
                    patch,
                }],
                omitted_files: 1,
                more_files_available: true,
            }),
        });
        app.apply(Command::Open);
        app
    }

    fn assert_notice_tail_navigation(app: &mut App, notice: &str) {
        let initial = rendered_text(app, 120, 16);
        assert!(initial.contains(notice));
        assert!(!initial.contains("LINE13"));

        app.handle_input(Input::Character('/'));
        for character in "MATCH".chars() {
            app.handle_input(Input::Character(character));
        }
        app.handle_input(Input::Enter);
        let searched = rendered_text(app, 120, 16);
        assert!(searched.contains(notice));
        assert!(searched.contains("LINE13"));

        app.handle_input(Input::Character('n'));
        assert!(rendered_text(app, 120, 16).contains("LINE01"));
        app.handle_input(Input::Character('N'));
        assert!(rendered_text(app, 120, 16).contains("LINE13"));

        app.apply(Command::Last);
        let tail = rendered_text(app, 120, 16);
        assert!(tail.contains(notice));
        assert!(tail.contains("LINE13"));
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

    #[test]
    fn fatal_authentication_missing_gh_permission_and_rate_limit_are_distinct() {
        for (category, expected) in [
            (FailureCategory::Authentication, "authentication failed"),
            (FailureCategory::MissingGh, "GitHub CLI not found"),
            (
                FailureCategory::PermissionOrNotFound,
                "permission denied or resource unavailable",
            ),
            (FailureCategory::RateLimit, "GitHub API rate limit reached"),
        ] {
            let output = rendered_text(&mut fatal_app(category), 120, 32);
            assert!(output.contains("LOAD FAILED"));
            assert!(output.contains(expected), "missing {expected:?}");
        }
    }

    #[test]
    fn complete_empty_day_and_no_owned_repositories_are_not_conflated() {
        let mut no_repositories = live_app();
        no_repositories.apply_load_event(LoadEvent::Finished {
            status: LoadStatus::Complete,
            progress: LoadProgress::default(),
        });
        let output = rendered_text(&mut no_repositories, 120, 32);
        assert!(output.contains("NO OWNED REPOSITORIES"));
        assert!(!output.contains("EMPTY DAY"));

        let mut empty_day = live_app();
        empty_day.apply_load_event(LoadEvent::RepositoryLoaded {
            repository_index: 0,
            repository: LoadedRepository {
                repository: Repository {
                    identity: RepositoryIdentity {
                        id: 1,
                        owner: "fixture".to_owned(),
                        name: "empty".to_owned(),
                    },
                    commits: Vec::new(),
                },
                branch_count: 1,
                coverage: RepositoryCoverage::Complete,
            },
        });
        empty_day.apply_load_event(LoadEvent::Finished {
            status: LoadStatus::Complete,
            progress: LoadProgress {
                repositories_discovered: 1,
                repositories_processed: 1,
                branches_discovered: 1,
                branches_processed: 1,
                commits_loaded: 0,
            },
        });
        let output = rendered_text(&mut empty_day, 120, 32);
        assert!(output.contains("EMPTY DAY"));
        assert!(!output.contains("NO OWNED REPOSITORIES"));
    }

    #[test]
    fn incomplete_outcome_warns_without_a_completion_claim() {
        let mut app = live_app();
        app.apply_load_event(LoadEvent::RepositoryLoaded {
            repository_index: 0,
            repository: LoadedRepository {
                repository: Repository {
                    identity: RepositoryIdentity {
                        id: 2,
                        owner: "fixture".to_owned(),
                        name: "partial".to_owned(),
                    },
                    commits: Vec::new(),
                },
                branch_count: 2,
                coverage: RepositoryCoverage::BranchesIncomplete { failed_branches: 1 },
            },
        });
        app.apply_load_event(LoadEvent::Failure(LoadFailure {
            category: FailureCategory::RateLimit,
            scope: FailureScope::Branch {
                repository_index: 0,
                branch_index: 1,
            },
            http_status: Some(403),
        }));
        app.apply_load_event(LoadEvent::Finished {
            status: LoadStatus::Incomplete,
            progress: LoadProgress {
                repositories_discovered: 1,
                repositories_processed: 1,
                branches_discovered: 2,
                branches_processed: 2,
                commits_loaded: 0,
            },
        });

        let output = rendered_text(&mut app, 120, 32);
        assert!(output.contains("INCOMPLETE RESULTS"));
        assert!(output.contains("rate limit was reached"));
        assert!(output.contains("incomplete coverage"));
        assert!(!output.contains("COMPLETE — daily inbox loaded"));
        assert!(!output.contains("EMPTY DAY"));
    }

    #[test]
    fn live_detail_states_are_explicit_in_the_unified_four_pane_layout() {
        let mut app = live_commit_app();
        app.apply(Command::Open);
        app.apply(Command::Open);
        let (request_id, key) = request_identity(&mut app);
        let loading = rendered_text(&mut app, 120, 32);
        for pane in ["Repository", "Commit", "File", "Diff"] {
            assert!(loading.contains(pane), "missing {pane}");
        }
        assert!(loading.contains("Loading commit details"));

        app.apply_detail_result(DetailResult {
            request_id,
            key,
            outcome: Err(DetailFailure::ResponseTruncated),
        });
        assert!(rendered_text(&mut app, 120, 32).contains(RESPONSE_TRUNCATED_LABEL));

        app.apply(Command::Back);
        app.apply(Command::Open);
        let (request_id, key) = request_identity(&mut app);
        app.apply_detail_result(DetailResult {
            request_id,
            key,
            outcome: Ok(CommitDetail {
                files: vec![FileChange {
                    path: "assets/image.bin".to_owned(),
                    api_path_is_commentable: true,
                    previous_path: None,
                    status: FileStatus::Modified,
                    additions: 0,
                    deletions: 0,
                    changes: 0,
                    patch: PatchContent::NoPatch,
                }],
                omitted_files: 0,
                more_files_available: false,
            }),
        });
        let unavailable = rendered_text(&mut app, 120, 32);
        assert!(unavailable.contains("assets/image.bin"));
        assert!(unavailable.contains(NO_PATCH_LABEL));
    }

    #[test]
    fn every_patch_cap_and_detail_failure_state_has_a_precise_presentation() {
        let states = [
            (PatchContent::Empty, "No textual changes"),
            (PatchContent::NoPatch, NO_PATCH_LABEL),
            (
                PatchContent::Unavailable,
                "Patch not provided by GitHub (binary or too large)",
            ),
            (
                PatchContent::Capped {
                    lines: vec![DiffLine {
                        kind: DiffLineKind::Addition,
                        old_line: None,
                        new_line: Some(1),
                        text: "+kept".to_owned(),
                    }],
                    omitted_lines: 14,
                    omitted_bytes: 1_025,
                    reason: PatchCapReason::FileLimit,
                },
                "Patch capped locally: 14 lines / 2 KiB omitted",
            ),
            (
                PatchContent::Capped {
                    lines: Vec::new(),
                    omitted_lines: 14,
                    omitted_bytes: 1_025,
                    reason: PatchCapReason::CommitBudget,
                },
                "Omitted by per-commit budget",
            ),
        ];
        for (patch, expected) in states {
            let mut app = demo_patch_app(patch);
            assert!(
                rendered_text(&mut app, 120, 32).contains(expected),
                "missing {expected}"
            );
        }

        let mut app = live_commit_app();
        app.apply(Command::Open);
        app.apply(Command::Open);
        let (request_id, key) = request_identity(&mut app);
        app.apply_detail_result(DetailResult {
            request_id,
            key,
            outcome: Err(DetailFailure::Load(LoadFailure {
                category: FailureCategory::Offline,
                scope: FailureScope::CommitDetail,
                http_status: None,
            })),
        });
        assert!(rendered_text(&mut app, 120, 32).contains("Commit details unavailable: offline"));

        let mut app = live_commit_app();
        app.apply(Command::Open);
        app.apply(Command::Open);
        let (request_id, key) = request_identity(&mut app);
        app.apply_detail_result(DetailResult {
            request_id,
            key,
            outcome: Ok(CommitDetail {
                files: vec![FileChange {
                    path: "src/kept.rs".to_owned(),
                    api_path_is_commentable: true,
                    previous_path: None,
                    status: FileStatus::Modified,
                    additions: 1,
                    deletions: 0,
                    changes: 1,
                    patch: PatchContent::Text {
                        lines: vec![DiffLine {
                            kind: DiffLineKind::Addition,
                            old_line: None,
                            new_line: Some(1),
                            text: "+kept".to_owned(),
                        }],
                    },
                }],
                omitted_files: 1,
                more_files_available: true,
            }),
        });
        assert!(rendered_text(&mut app, 120, 32).contains("Only first 300 files shown"));
    }

    #[test]
    fn demo_response_truncation_uses_the_live_label_at_minimum_full_size() {
        let mut app = App::new(Inbox::demo(vec![Repository {
            identity: RepositoryIdentity {
                id: 92,
                owner: "fixture".to_owned(),
                name: "truncated".to_owned(),
            },
            commits: vec![Commit {
                sha: "cccccccccccccccccccccccccccccccccccccccc".to_owned(),
                subject: "render truncated response".to_owned(),
                author: GitHubAuthor {
                    login: "fixture-user".to_owned(),
                },
                authored_at: Utc.with_ymd_and_hms(2024, 1, 15, 12, 0, 0).unwrap(),
                files: ChildPane::ResponseTruncated,
            }],
        }]));

        let output = rendered_text(&mut app, MIN_FULL_WIDTH, MIN_FULL_HEIGHT);
        assert!(output.contains("GitHub response exceeded 16 MiB;"));
        assert!(output.contains("details unavailable"));
    }

    #[test]
    fn long_diff_lines_wrap_resize_and_reach_the_tail_without_panicking() {
        let mut app = demo_patch_app(PatchContent::Text {
            lines: vec![DiffLine {
                kind: DiffLineKind::Addition,
                old_line: None,
                new_line: Some(10_000),
                text: format!("+{}TAIL", "x".repeat(600)),
            }],
        });
        for (width, height) in [(1, 1), (60, 16), (200, 60)] {
            let _ = rendered_text(&mut app, width, height);
        }
        app.apply(Command::Open);
        app.apply(Command::Open);
        app.apply(Command::Open);
        let head = rendered_text(&mut app, 60, 16);
        assert!(head.contains("10000"));
        app.apply(Command::Last);
        let tail = rendered_text(&mut app, 60, 16);
        assert!(tail.contains("TAIL"));
    }

    #[test]
    fn odd_width_diff_layout_preserves_wrapped_characters_and_true_tail() {
        for width in [61, 79, 120] {
            let mut visible = demo_patch_app(PatchContent::Text {
                lines: vec![DiffLine {
                    kind: DiffLineKind::Addition,
                    old_line: None,
                    new_line: Some(1),
                    text: format!("+{}", "#".repeat(300)),
                }],
            });
            let output = rendered_text(&mut visible, width, 40);
            assert_eq!(
                output.matches('#').count(),
                300,
                "wrapped characters were clipped at width {width}"
            );

            let mut tail = demo_patch_app(PatchContent::Text {
                lines: vec![DiffLine {
                    kind: DiffLineKind::Addition,
                    old_line: None,
                    new_line: Some(1),
                    text: format!("+{}TRUE-TAIL", "x".repeat(1_200)),
                }],
            });
            rendered_text(&mut tail, width, 16);
            tail.apply(Command::Open);
            tail.apply(Command::Open);
            tail.apply(Command::Open);
            tail.apply(Command::Last);
            assert!(
                rendered_text(&mut tail, width, 16).contains("TRUE-TAIL"),
                "G did not reach the true wrapped tail at width {width}"
            );
        }
    }

    #[test]
    fn wrapped_capped_notice_preserves_omitted_size_at_supported_widths() {
        let label = "Patch capped locally: 1234 lines / 300 KiB omitted";
        for width in [60, 80] {
            let mut app = demo_patch_app(PatchContent::Capped {
                lines: notice_patch_lines(),
                omitted_lines: 1_234,
                omitted_bytes: 300 * 1024,
                reason: PatchCapReason::FileLimit,
            });
            let notice_rows = wrap_text(
                label,
                ReviewPaneLayout::from_area(Rect::new(0, 0, width, 24)).diff_content_width(),
            )
            .len();
            let output = rendered_diff_prefix(&mut app, width, 24, notice_rows);
            assert!(output.contains(label), "width {width}: {output:?}");
            assert_eq!(app.diff_notice_texts(), vec![label]);
        }
    }

    #[test]
    fn file_list_truncation_is_visible_for_every_selected_patch_state() {
        let cases = [
            (PatchContent::Empty, "No textual changes"),
            (PatchContent::NoPatch, NO_PATCH_LABEL),
            (
                PatchContent::Unavailable,
                "Patch not provided by GitHub (binary or too large)",
            ),
            (
                PatchContent::Capped {
                    lines: Vec::new(),
                    omitted_lines: 12,
                    omitted_bytes: 1_024,
                    reason: PatchCapReason::CommitBudget,
                },
                "Omitted by per-commit budget",
            ),
        ];

        for (patch, patch_label) in cases {
            let mut app = live_file_notice_app(patch);
            let output = rendered_text(&mut app, 120, 32);
            assert!(
                output.contains("Only first 300 files shown"),
                "missing file-list disclosure for {patch_label}: {output:?}"
            );
            assert!(
                output.contains(patch_label),
                "missing {patch_label}: {output:?}"
            );
        }

        let mut file_capped = live_file_notice_app(PatchContent::Capped {
            lines: notice_patch_lines(),
            omitted_lines: 1_234,
            omitted_bytes: 300 * 1024,
            reason: PatchCapReason::FileLimit,
        });
        let output = rendered_text(&mut file_capped, 120, 32);
        assert!(output.contains("Only first 300 files shown"));
        assert!(output.contains("Patch capped locally: 1234 lines / 300 KiB omitted"));
    }

    #[test]
    fn exact_fit_diff_tail_is_reachable_below_all_notices() {
        let mut capped = demo_patch_app(PatchContent::Capped {
            lines: notice_patch_lines(),
            omitted_lines: 1_234,
            omitted_bytes: 300 * 1024,
            reason: PatchCapReason::FileLimit,
        });
        capped.apply(Command::Open);
        capped.apply(Command::Open);
        capped.apply(Command::Open);
        assert_notice_tail_navigation(&mut capped, "Patch capped locally");

        let mut files_omitted = live_file_notice_app(PatchContent::Text {
            lines: notice_patch_lines(),
        });
        assert_notice_tail_navigation(&mut files_omitted, "Only first 300 files shown");

        let mut both = live_file_notice_app(PatchContent::Capped {
            lines: notice_patch_lines(),
            omitted_lines: 1_234,
            omitted_bytes: 300 * 1024,
            reason: PatchCapReason::FileLimit,
        });
        let initial = rendered_text(&mut both, 120, 16);
        assert!(initial.contains("Only first 300 files shown"));
        assert!(initial.contains("Patch capped locally"));
        assert_notice_tail_navigation(&mut both, "Only first 300 files shown");
    }

    #[test]
    fn reviewed_marker_filter_empty_state_and_store_warning_are_visible() {
        let mut app = live_commit_app();
        app.apply(Command::Open);
        app.apply(Command::ToggleReviewed);
        assert!(rendered_text(&mut app, 120, 32).contains("✓"));
        app.apply(Command::ToggleRemaining);
        assert!(rendered_text(&mut app, 120, 32).contains("ALL REVIEWED"));

        let inbox = live_app().inbox().clone();
        let mut unavailable =
            App::with_review_store_result(inbox, Err(ReviewStateError::Unavailable));
        let output = rendered_text(&mut unavailable, 160, 32);
        assert!(output.contains("REVIEW STATE WARNING"));
        assert!(output.contains("Selected day: 2024-01-15"));
    }

    #[test]
    fn edit_mode_renders_target_state_and_cursor_context_in_full_and_compact_layouts() {
        let mut app = App::new(DemoFixture::load());
        app.handle_input(Input::Character('l'));
        app.handle_input(Input::Character('c'));
        for character in "first\nsecond".chars() {
            app.handle_input(if character == '\n' {
                Input::Enter
            } else {
                Input::Character(character)
            });
        }

        let full = rendered_text(&mut app, 120, 32);
        assert!(full.contains("EDIT comment"));
        assert!(full.contains("Target: commit a1b2c3d"));
        assert!(full.contains("New draft"));
        assert!(full.contains("first"));
        assert!(full.contains("second"));
        assert!(full.contains("save/return"));

        let backend = TestBackend::new(120, 32);
        let mut terminal = Terminal::new(backend).expect("test terminal");
        app.resize(120, 32);
        terminal.draw(|frame| draw(frame, &app)).expect("draw");
        let cursor = terminal.get_cursor_position().expect("cursor position");
        assert!(cursor.x > 0 && cursor.y > 0);

        let compact = rendered_text(&mut app, 40, 8);
        assert!(compact.contains("EDIT comment"));
        assert!(compact.contains("Target: commit a1b2c3d"));
        assert!(compact.contains("New draft"));

        for (width, height) in [(1, 1), (2, 2), (10, 3), (59, 15), (120, 32)] {
            let _ = rendered_text(&mut app, width, height);
            assert!(matches!(app.mode(), Mode::Edit { .. }));
        }
    }

    #[test]
    fn diff_cursor_eligibility_and_commit_and_line_draft_markers_are_visible() {
        let mut app = App::new(DemoFixture::load());
        app.handle_input(Input::Character('l'));
        app.handle_input(Input::Character('c'));
        app.handle_input(Input::Character('x'));
        app.handle_input(Input::Escape);
        assert!(rendered_text(&mut app, 120, 32).contains("◆"));

        app.handle_input(Input::Character('l'));
        app.handle_input(Input::Character('l'));
        let ineligible = rendered_text(&mut app, 140, 32);
        assert!(ineligible.contains("line comments unavailable: hunk header"));
        app.handle_input(Input::Character('j'));
        let eligible = rendered_text(&mut app, 140, 32);
        assert!(eligible.contains("commentable src/welcome.rs:1"));
        app.handle_input(Input::Character('c'));
        let editor = rendered_text(&mut app, 140, 32);
        assert!(editor.contains("src/welcome.rs:1"));
        assert!(editor.contains("old 1, new 1"));
        app.handle_input(Input::Character('y'));
        app.handle_input(Input::Escape);
        let drafted = rendered_text(&mut app, 140, 32);
        assert!(drafted.contains("◆"));
        assert!(drafted.contains("commentable src/welcome.rs:1"));
    }
}
