use std::io;

use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::layout::Rect;

use crate::app::{App, Input, Mode, Pane};
use crate::fixture::DemoFixture;
use crate::render;

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
    let mut app = App::new(fixture);
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
    input(&mut app, 'k');
    ensure(app.selected(Pane::Repository) == 0, "k must move up")?;

    input(&mut app, 'G');
    ensure(
        app.selected(Pane::Repository) == app.fixture().repositories.len() - 1,
        "G must move to the last repository",
    )?;
    ensure_contains(
        &render_frame(&mut terminal, &mut app, &mut frames)?,
        "fictional-foundry/quiet-signal-demo",
        "G navigation frame",
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

    app.handle_input(Input::HalfPageDown);
    ensure(
        app.selected(Pane::Repository) > 0,
        "Ctrl-d must move by a bounded half page",
    )?;
    ensure_contains(
        &render_frame(&mut terminal, &mut app, &mut frames)?,
        "Moved down",
        "half-page navigation frame",
    )?;
    app.handle_input(Input::HalfPageUp);
    ensure(
        app.selected(Pane::Repository) == 0,
        "Ctrl-u must move back toward the first item",
    )?;

    app.handle_input(Input::Enter);
    ensure(
        app.focus() == Pane::Commit,
        "Enter must open the child pane",
    )?;
    input(&mut app, 'l');
    ensure(app.focus() == Pane::File, "l must focus the next pane")?;
    input(&mut app, 'h');
    ensure(
        app.focus() == Pane::Commit,
        "h must focus the previous pane",
    )?;
    app.handle_input(Input::Escape);
    ensure(
        app.focus() == Pane::Repository,
        "Escape must return to the parent pane",
    )?;
    app.handle_input(Input::Enter);
    app.handle_input(Input::Enter);
    ensure(app.focus() == Pane::File, "Enter must descend to files")?;

    input(&mut app, '/');
    for character in "routes".chars() {
        input(&mut app, character);
    }
    ensure(
        app.mode() == Mode::SearchEntry { target: Pane::File },
        "slash must enter search mode for the focused pane",
    )?;
    ensure_contains(
        &render_frame(&mut terminal, &mut app, &mut frames)?,
        "/routes",
        "search entry frame",
    )?;
    app.handle_input(Input::Enter);
    ensure(
        app.mode() == Mode::Normal
            && app.current_file().map(|file| file.path) == Some("src/routes.rs"),
        "Enter must apply search and select the matching fixture file",
    )?;
    ensure_contains(
        &render_frame(&mut terminal, &mut app, &mut frames)?,
        "Match for 'routes'",
        "applied search frame",
    )?;

    input(&mut app, '?');
    ensure(
        matches!(app.mode(), Mode::Help { .. }),
        "question mark must open help",
    )?;
    ensure_contains(
        &render_frame(&mut terminal, &mut app, &mut frames)?,
        "Keyboard help",
        "help frame",
    )?;
    app.handle_input(Input::Escape);
    ensure(
        app.mode() == Mode::Normal && app.focus() == Pane::File,
        "Escape must close help and restore focus",
    )?;

    app.handle_input(Input::Enter);
    ensure(app.focus() == Pane::Diff, "Enter must open the diff pane")?;
    resize(&mut terminal, 60, 16)?;
    let resized = render_frame(&mut terminal, &mut app, &mut frames)?;
    ensure_contains(&resized, "Diff", "resized full layout")?;
    input(&mut app, 'G');
    ensure(app.scroll(Pane::Diff) > 0, "G must scroll to the diff end")?;
    ensure_contains(
        &render_frame(&mut terminal, &mut app, &mut frames)?,
        "Moved to last position",
        "diff-end navigation frame",
    )?;
    input(&mut app, 'g');
    input(&mut app, 'g');
    ensure(
        app.scroll(Pane::Diff) == 0,
        "gg must scroll to the diff start",
    )?;
    app.handle_input(Input::HalfPageDown);
    ensure(app.scroll(Pane::Diff) > 0, "Ctrl-d must scroll the diff")?;
    app.handle_input(Input::HalfPageUp);
    ensure(
        app.scroll(Pane::Diff) == 0,
        "Ctrl-u must scroll the diff up",
    )?;

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
