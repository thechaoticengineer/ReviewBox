use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};

use crate::app::{App, HELP_BINDINGS, LivePhase, MIN_FULL_HEIGHT, MIN_FULL_WIDTH, Mode, Pane};
use crate::day::TimezoneSource;
use crate::github::{DetailFailure, DetailState, FailureCategory};
use crate::inbox::{DiffLineKind, InboxSource, PatchContent};

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
    draw_review_panes(frame, rows[0], app);
    draw_status(frame, rows[1], app);
}

fn draw_review_panes(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(44), Constraint::Percentage(56)])
        .split(area);
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
        left[1],
        Pane::Commit,
        app,
        app.current_commits()
            .iter()
            .map(|commit| {
                let reviewed = app
                    .current_repository()
                    .is_some_and(|repository| app.is_reviewed(repository.identity.id, &commit.sha));
                let marker = if reviewed { "✓ " } else { "  " };
                format!("{marker}{}", sanitize_display_text(&commit.label()))
            })
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
            .map(|file| sanitize_display_text(&file.path))
            .collect(),
        "files",
    );
    draw_diff_pane(frame, columns[1], app);
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
        vec![Line::styled(
            "  Loading commit details…",
            Style::default().fg(Color::Cyan),
        )]
    } else if let Some(DetailState::Failed(failure)) = app.current_detail_state() {
        let label = match failure {
            DetailFailure::ResponseTruncated => {
                "ResponseTruncated — commit response exceeded the local size limit".to_owned()
            }
            DetailFailure::Load(failure) => format!("Failed — {failure}"),
        };
        vec![Line::styled(
            format!("  {label}"),
            Style::default().fg(Color::Red),
        )]
    } else if let Some(file) = app.current_file() {
        match &file.patch {
            PatchContent::Empty => vec![Line::styled(
                "  Empty patch — GitHub returned no diff lines",
                Style::default().fg(Color::DarkGray),
            )],
            PatchContent::Unavailable => vec![Line::styled(
                "  Unavailable — binary or API-omitted patch",
                Style::default().fg(Color::Yellow),
            )],
            PatchContent::Text { .. } | PatchContent::Capped { .. } => diff_lines(app),
        }
    } else if matches!(app.current_detail_state(), Some(DetailState::Ready(_))) {
        vec![Line::styled(
            "  (no changed files returned for this commit)",
            Style::default().fg(Color::DarkGray),
        )]
    } else if app.current_diff_lines().is_empty() {
        vec![Line::styled(
            "  (no diff content in this fixture)",
            Style::default().fg(Color::DarkGray),
        )]
    } else {
        diff_lines(app)
    };

    frame.render_widget(
        Paragraph::new(lines).block(pane_block(Pane::Diff, app.focus() == Pane::Diff, app)),
        area,
    );
}

fn diff_lines(app: &App) -> Vec<Line<'static>> {
    let mut lines: Vec<_> = app
        .current_diff_lines()
        .iter()
        .enumerate()
        .skip(app.scroll(Pane::Diff))
        .map(|(index, content)| {
            let color = match content.kind {
                DiffLineKind::Hunk => Color::Cyan,
                DiffLineKind::Addition => Color::Green,
                DiffLineKind::Deletion => Color::Red,
                _ => Color::Reset,
            };
            Line::from(vec![
                Span::styled(
                    format!("{:>4} ", index + 1),
                    Style::default().fg(Color::DarkGray),
                ),
                Span::styled(content.text.clone(), Style::default().fg(color)),
            ])
        })
        .collect();
    if let Some(file) = app.current_file()
        && let PatchContent::Capped {
            omitted_lines,
            omitted_bytes,
            ..
        } = &file.patch
    {
        lines.insert(
            0,
            Line::styled(
                format!(
                    "… locally truncated: {omitted_lines} lines and {omitted_bytes} bytes omitted"
                ),
                Style::default().fg(Color::Yellow),
            ),
        );
    }
    if let Some(DetailState::Ready(detail)) = app.current_detail_state()
        && (detail.omitted_files > 0 || detail.more_files_available)
    {
        let suffix = if detail.more_files_available {
            "; additional GitHub pages are unavailable"
        } else {
            ""
        };
        lines.insert(
            0,
            Line::styled(
                format!(
                    "… incomplete file list: {} files omitted{suffix}",
                    detail.omitted_files
                ),
                Style::default().fg(Color::Yellow),
            ),
        );
    }
    lines
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
    let mut spans = vec![
        Span::styled(mode, Style::default().fg(Color::Black).bg(Color::Cyan)),
        Span::raw(format!(
            " {} • {} • h/l focus • j/k move • m reviewed • f remaining • / search • ? help • q quit",
            app.focus().title(),
            app.status()
        )),
    ];
    if let Some(warning) = app.review_warning() {
        spans.push(Span::styled(
            format!(" • REVIEW STATE WARNING: {warning}"),
            Style::default().fg(Color::Yellow),
        ));
    }
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
    };
    let warning = app
        .review_warning()
        .map(|warning| format!("\nREVIEW STATE WARNING: {warning}"))
        .unwrap_or_default();
    let message = Paragraph::new(format!(
        "{mode} • {}\nterminal too small for panes\nresize to at least 60×16\nm reviewed • f remaining • / search • ? help • q quit{warning}",
        app.focus().title(),
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
    use crate::app::{Command, DetailEffect, DetailResult, Input};
    use crate::fixture::DemoFixture;
    use crate::github::{
        FailureScope, LoadEvent, LoadFailure, LoadProgress, LoadStatus, LoadedRepository,
        RepositoryCoverage,
    };
    use crate::inbox::{
        ChildPane, Commit, CommitDetail, FileChange, FileStatus, GitHubAuthor, Inbox, PatchContent,
        Repository, RepositoryIdentity,
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
        assert!(rendered_text(&mut app, 120, 32).contains("ResponseTruncated"));

        app.apply(Command::Back);
        app.apply(Command::Open);
        let (request_id, key) = request_identity(&mut app);
        app.apply_detail_result(DetailResult {
            request_id,
            key,
            outcome: Ok(CommitDetail {
                files: vec![FileChange {
                    path: "assets/image.bin".to_owned(),
                    previous_path: None,
                    status: FileStatus::Modified,
                    additions: 0,
                    deletions: 0,
                    changes: 0,
                    patch: PatchContent::Unavailable,
                }],
                omitted_files: 0,
                more_files_available: false,
            }),
        });
        let unavailable = rendered_text(&mut app, 120, 32);
        assert!(unavailable.contains("assets/image.bin"));
        assert!(unavailable.contains("Unavailable"));
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
}
