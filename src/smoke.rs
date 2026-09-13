use std::io;

use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::layout::Rect;

use crate::app::{App, Input, Mode, Pane};
use crate::fixture::DemoFixture;
use crate::render;
use crate::review_state::MemoryReviewStore;

const FULL_WIDTH: u16 = 120;
const FULL_HEIGHT: u16 = 32;

pub struct SmokeReport {
    pub frames: usize,
}

pub fn run() -> io::Result<SmokeReport> {
    let fixture = DemoFixture::load();
    ensure(
        fixture.repositories.len() >= 2,
        "fixture must contain representative repositories",
    )?;

    let backend = TestBackend::new(FULL_WIDTH, FULL_HEIGHT);
    let mut terminal = Terminal::new(backend).map_err(io::Error::other)?;
    let mut app = App::with_review_store(fixture, Box::new(MemoryReviewStore::default()));
    ensure(
        app.fixture().repositories.len() >= 2,
        "application must retain the demo fixture",
    )?;
    let mut frames = 0;

    let initial = render_frame(&mut terminal, &mut app, &mut frames)?;
    ensure_contains(&initial, "Repository", "initial repository pane")?;
    ensure_contains(&initial, "Commit", "initial commit pane")?;
    ensure_contains(&initial, "File", "initial file pane")?;
    ensure_contains(&initial, "Diff", "initial diff pane")?;
    ensure_contains(
        &initial,
        "fictional-labs/orbit-notes-demo",
        "production demo fixture",
    )?;

    input(&mut app, 'j');
    ensure(app.selected(Pane::Repository) == 1, "j must move down")?;
    ensure_contains(
        &render_frame(&mut terminal, &mut app, &mut frames)?,
        "fictional-studio/pixel-garden-demo",
        "j navigation frame",
    )?;
    input(&mut app, 'g');
    input(&mut app, 'g');
    ensure(
        app.selected(Pane::Repository) == 0 && !app.pending_g(),
        "gg must move to the first repository and clear its prefix",
    )?;
    ensure_contains(
        &render_frame(&mut terminal, &mut app, &mut frames)?,
        "fictional-labs/orbit-notes-demo",
        "gg navigation frame",
    )?;

    app.handle_input(Input::Enter);
    ensure(
        app.focus() == Pane::Commit,
        "Enter must open the commit pane",
    )?;
    ensure_contains(
        &render_frame(&mut terminal, &mut app, &mut frames)?,
        "Refine fictional launch screen",
        "commit navigation frame",
    )?;
    app.handle_input(Input::Enter);
    ensure(app.focus() == Pane::File, "Enter must descend to files")?;
    ensure_contains(
        &render_frame(&mut terminal, &mut app, &mut frames)?,
        "src/welcome.rs",
        "file navigation frame",
    )?;
    app.handle_input(Input::Enter);
    ensure(app.focus() == Pane::Diff, "Enter must open the diff pane")?;
    ensure_contains(
        &render_frame(&mut terminal, &mut app, &mut frames)?,
        "Welcome aboard",
        "opened substantial diff frame",
    )?;

    resize(&mut terminal, 60, 16)?;
    ensure(
        app.current_diff_lines().len() > 32,
        "fixture diff must exceed the 60x16 viewport",
    )?;
    ensure_contains(
        &render_frame(&mut terminal, &mut app, &mut frames)?,
        "Diff",
        "60x16 diff frame",
    )?;

    input(&mut app, 'G');
    ensure(app.scroll(Pane::Diff) > 0, "G must scroll to the diff end")?;
    let tail = render_frame(&mut terminal, &mut app, &mut frames)?;
    ensure_contains(&tail, "launch revie", "diff tail frame")?;
    input(&mut app, 'g');
    input(&mut app, 'g');
    ensure(
        app.scroll(Pane::Diff) == 0,
        "gg must scroll to the diff start",
    )?;
    let head = render_frame(&mut terminal, &mut app, &mut frames)?;
    ensure_contains(&head, "@@ -1,12 +1,27 @@", "diff head frame")?;

    resize(&mut terminal, FULL_WIDTH, FULL_HEIGHT)?;
    input(&mut app, '/');
    for character in "beacon".chars() {
        input(&mut app, character);
    }
    ensure(
        app.mode() == Mode::SearchEntry { target: Pane::Diff },
        "slash must enter diff search mode",
    )?;
    ensure_contains(
        &render_frame(&mut terminal, &mut app, &mut frames)?,
        "/beacon",
        "diff search-entry frame",
    )?;
    app.handle_input(Input::Enter);
    ensure(
        app.mode() == Mode::Normal && app.status().contains("Match for 'beacon'"),
        "Enter must apply diff search",
    )?;
    ensure_contains(
        &render_frame(&mut terminal, &mut app, &mut frames)?,
        "Match for 'beacon'",
        "applied diff search frame",
    )?;
    input(&mut app, 'n');
    ensure_contains(
        &render_frame(&mut terminal, &mut app, &mut frames)?,
        "next match for 'beacon'",
        "forward repeated search frame",
    )?;
    for _ in 0..app.current_diff_lines().len() {
        if app.status().contains("wrapped") {
            break;
        }
        input(&mut app, 'n');
    }
    ensure(app.status().contains("wrapped"), "n must wrap forward")?;
    ensure_contains(
        &render_frame(&mut terminal, &mut app, &mut frames)?,
        "(wrapped)",
        "wrapped forward search frame",
    )?;
    input(&mut app, 'N');
    ensure(
        app.status().contains("previous match") && app.status().contains("wrapped"),
        "N must wrap backward",
    )?;
    ensure_contains(
        &render_frame(&mut terminal, &mut app, &mut frames)?,
        "previous match for 'beacon'",
        "wrapped backward search frame",
    )?;

    input(&mut app, 'm');
    ensure_contains(
        &render_frame(&mut terminal, &mut app, &mut frames)?,
        "✓",
        "reviewed marker frame",
    )?;
    input(&mut app, 'm');
    let unreviewed = render_frame(&mut terminal, &mut app, &mut frames)?;
    ensure_not_contains(&unreviewed, "✓", "unreviewed marker frame")?;
    ensure_contains(
        &unreviewed,
        "Marked commit unreviewed",
        "unreviewed status frame",
    )?;

    app.handle_input(Input::Escape);
    search(&mut app, "fictional-orbit-map.bin")?;
    ensure(
        app.current_file().map(|file| file.path.as_str()) == Some("assets/fictional-orbit-map.bin"),
        "file search must select the unavailable binary fixture",
    )?;
    app.handle_input(Input::Enter);
    ensure_contains(
        &render_frame(&mut terminal, &mut app, &mut frames)?,
        "Patch not provided by GitHub (binary or too large)",
        "unavailable patch frame",
    )?;

    app.handle_input(Input::Escape);
    search(&mut app, "fictional-catalog.rs")?;
    app.handle_input(Input::Enter);
    ensure_contains(
        &render_frame(&mut terminal, &mut app, &mut frames)?,
        "Patch capped locally",
        "locally capped patch frame",
    )?;

    app.handle_input(Input::Escape);
    app.handle_input(Input::Escape);
    input(&mut app, 'j');
    ensure(
        app.current_commit()
            .is_some_and(|commit| commit.subject == "Simulate a fictional oversized response"),
        "commit navigation must select the truncated-detail fixture",
    )?;
    ensure_contains(
        &render_frame(&mut terminal, &mut app, &mut frames)?,
        "GitHub response exceeded 16 MiB; details unavailable",
        "truncated response frame",
    )?;

    input(&mut app, 'm');
    ensure_contains(
        &render_frame(&mut terminal, &mut app, &mut frames)?,
        "✓",
        "reviewed truncated commit frame",
    )?;
    input(&mut app, 'f');
    let remaining = render_frame(&mut terminal, &mut app, &mut frames)?;
    ensure_contains(
        &remaining,
        "Showing remaining commits only",
        "remaining-only filter frame",
    )?;
    ensure_not_contains(
        &remaining,
        "Simulate a fictional oversized response",
        "remaining-only filter frame",
    )?;

    let (_, total) = app.review_progress();
    for _ in 0..total {
        if app.all_reviewed_empty() {
            break;
        }
        input(&mut app, 'm');
    }
    ensure(
        app.all_reviewed_empty() && app.review_progress() == (total, total),
        "marking every remaining commit must reach all-reviewed state",
    )?;
    ensure_contains(
        &render_frame(&mut terminal, &mut app, &mut frames)?,
        "ALL REVIEWED",
        "all-reviewed frame",
    )?;

    input(&mut app, '?');
    ensure(
        matches!(app.mode(), Mode::Help { .. }),
        "question mark must open help",
    )?;
    let help = render_frame(&mut terminal, &mut app, &mut frames)?;
    for binding in [
        "n / N",
        "mark commit reviewed / unreviewed",
        "show remaining / all commits",
    ] {
        ensure_contains(&help, binding, "help frame")?;
    }
    app.handle_input(Input::Escape);
    ensure(app.mode() == Mode::Normal, "Escape must close help")?;

    input(&mut app, 'f');
    let restored = render_frame(&mut terminal, &mut app, &mut frames)?;
    ensure_contains(
        &restored,
        "Refine fictional launch screen",
        "restored all-commits frame",
    )?;
    ensure_contains(&restored, "all", "restored filter label")?;

    resize(&mut terminal, 40, 8)?;
    ensure_contains(
        &render_frame(&mut terminal, &mut app, &mut frames)?,
        "terminal too small",
        "compact resize frame",
    )?;

    input(&mut app, 'q');
    ensure(app.should_quit(), "q must request a clean normal-mode exit")?;

    Ok(SmokeReport { frames })
}

fn search(app: &mut App, query: &str) -> io::Result<()> {
    input(app, '/');
    for character in query.chars() {
        input(app, character);
    }
    ensure(
        matches!(app.mode(), Mode::SearchEntry { .. }),
        "slash must enter search mode",
    )?;
    app.handle_input(Input::Enter);
    ensure(
        app.mode() == Mode::Normal && app.status().contains("Match for"),
        "search must find its fictional target",
    )
}

fn input(app: &mut App, character: char) {
    app.handle_input(Input::Character(character));
}

fn resize(terminal: &mut Terminal<TestBackend>, width: u16, height: u16) -> io::Result<()> {
    terminal.backend_mut().resize(width, height);
    terminal
        .resize(Rect::new(0, 0, width, height))
        .map_err(io::Error::other)
}

fn render_frame(
    terminal: &mut Terminal<TestBackend>,
    app: &mut App,
    frames: &mut usize,
) -> io::Result<String> {
    terminal
        .draw(|frame| {
            let area = frame.area();
            app.resize(area.width, area.height);
            render::draw(frame, app);
        })
        .map_err(io::Error::other)?;
    *frames += 1;

    Ok(terminal
        .backend()
        .buffer()
        .content
        .iter()
        .map(|cell| cell.symbol())
        .collect())
}

fn ensure(condition: bool, message: &str) -> io::Result<()> {
    if condition {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "smoke invariant failed: {message}"
        )))
    }
}

fn ensure_contains(frame: &str, expected: &str, context: &str) -> io::Result<()> {
    ensure(
        frame.contains(expected),
        &format!("{context} must contain {expected:?}"),
    )
}

fn ensure_not_contains(frame: &str, unexpected: &str, context: &str) -> io::Result<()> {
    ensure(
        !frame.contains(unexpected),
        &format!("{context} must not contain {unexpected:?}"),
    )
}
