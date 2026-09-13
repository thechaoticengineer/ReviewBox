use std::collections::VecDeque;
use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use chrono::{TimeZone, Utc};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::layout::Rect;

use crate::app::{App, AttemptSource, Input, Mode, Pane};
use crate::cli::{self, Command};
use crate::comment_draft::{MemoryDraftStore, SubmissionAttempt};
use crate::day::LocalTimezoneDetector;
use crate::event::{self, AppEvent, CommentRequester, EventSource, FakeComments};
use crate::external_editor::{
    EditorCommand, EditorError, EditorOutcome, EditorProcess, EditorTempFiles, edit_draft,
};
use crate::fixture::DemoFixture;
use crate::github::{CommentFailure, CommentFailureKind, PublishOutcome, RESPONSE_TRUNCATED_LABEL};
use crate::inbox::Inbox;
use crate::render;
use crate::render::NO_PATCH_LABEL;
use crate::review_state::MemoryReviewStore;
use crate::terminal::{TerminalGuard, TerminalOps};

const FULL_WIDTH: u16 = 120;
const FULL_HEIGHT: u16 = 32;
const FIXED_ATTEMPTS: [&str; 2] = [
    "0123456789abcdef0123456789abcdef",
    "fedcba9876543210fedcba9876543210",
];
const EDITOR_REPLACEMENT: &str = "Fictional external-editor replacement.";

#[derive(Debug, Default)]
struct SmokeAttemptSource(AtomicUsize);

impl AttemptSource for SmokeAttemptSource {
    fn next(&self) -> Result<SubmissionAttempt, ()> {
        let index = self.0.fetch_add(1, Ordering::Relaxed);
        SubmissionAttempt::new(
            FIXED_ATTEMPTS.get(index).copied().ok_or(())?,
            Utc.with_ymd_and_hms(2024, 1, 15, 12, 3, 0).unwrap(),
        )
        .map_err(|_| ())
    }

    fn now(&self) -> chrono::DateTime<Utc> {
        Utc.with_ymd_and_hms(2024, 1, 15, 12, 3, 0).unwrap()
    }
}

struct RecordingTerminalOps(Arc<Mutex<Vec<&'static str>>>);

impl RecordingTerminalOps {
    fn record(&self, operation: &'static str) {
        self.0.lock().expect("smoke terminal log").push(operation);
    }
}

struct SmokeTimezoneDetector(Result<String, String>);

impl LocalTimezoneDetector for SmokeTimezoneDetector {
    fn detect(&self) -> Result<String, String> {
        self.0.clone()
    }
}

struct ScriptedEvents(VecDeque<AppEvent>);

impl EventSource for ScriptedEvents {
    fn poll(&mut self, _timeout: std::time::Duration) -> io::Result<Option<AppEvent>> {
        self.0
            .pop_front()
            .map(Some)
            .ok_or_else(|| io::Error::other("smoke event script exhausted"))
    }
}

struct NeverEditor;

impl EditorProcess for NeverEditor {
    fn run(&mut self, _command: &EditorCommand, _path: &Path) -> Result<bool, EditorError> {
        Err(EditorError::Launch)
    }
}

impl TerminalOps for RecordingTerminalOps {
    fn enable_raw_mode(&mut self) -> io::Result<()> {
        self.record("enable_raw_mode");
        Ok(())
    }

    fn enter_alternate_screen(&mut self) -> io::Result<()> {
        self.record("enter_alternate_screen");
        Ok(())
    }

    fn hide_cursor(&mut self) -> io::Result<()> {
        self.record("hide_cursor");
        Ok(())
    }

    fn show_cursor(&mut self) -> io::Result<()> {
        self.record("show_cursor");
        Ok(())
    }

    fn leave_alternate_screen(&mut self) -> io::Result<()> {
        self.record("leave_alternate_screen");
        Ok(())
    }

    fn disable_raw_mode(&mut self) -> io::Result<()> {
        self.record("disable_raw_mode");
        Ok(())
    }
}

struct MemoryEditorFiles {
    body: Arc<Mutex<String>>,
    log: Arc<Mutex<Vec<&'static str>>>,
}

impl EditorTempFiles for MemoryEditorFiles {
    fn create(&mut self, body: &str) -> Result<PathBuf, EditorError> {
        self.log.lock().expect("smoke editor log").push("create");
        *self.body.lock().expect("smoke editor body") = body.to_owned();
        Ok(PathBuf::from("/fictional-private/draft.md"))
    }

    fn read(&mut self, _path: &Path) -> Result<String, EditorError> {
        self.log.lock().expect("smoke editor log").push("read");
        Ok(self.body.lock().expect("smoke editor body").clone())
    }

    fn cleanup(&mut self, _path: &Path) -> Result<(), EditorError> {
        self.log.lock().expect("smoke editor log").push("cleanup");
        Ok(())
    }
}

struct ReplacingEditor {
    body: Arc<Mutex<String>>,
    log: Arc<Mutex<Vec<&'static str>>>,
    runs: usize,
}

impl EditorProcess for ReplacingEditor {
    fn run(&mut self, command: &EditorCommand, path: &Path) -> Result<bool, EditorError> {
        self.log.lock().expect("smoke editor log").push("launch");
        self.runs = self.runs.saturating_add(1);
        if command.program != "nvim"
            || command.args != ["--clean"]
            || path != Path::new("/fictional-private/draft.md")
        {
            return Err(EditorError::Launch);
        }
        *self.body.lock().expect("smoke editor body") = EDITOR_REPLACEMENT.to_owned();
        Ok(true)
    }
}

pub struct SmokeReport {
    pub frames: usize,
}

pub fn run() -> io::Result<SmokeReport> {
    let mut frames = exercise_selected_day_presentation()?;

    let fixture = DemoFixture::load();
    ensure(
        fixture.repositories.len() >= 2,
        "fixture must contain representative repositories",
    )?;

    let backend = TestBackend::new(FULL_WIDTH, FULL_HEIGHT);
    let mut terminal = Terminal::new(backend).map_err(io::Error::other)?;
    let mut app = App::with_stores(
        fixture,
        Box::new(MemoryReviewStore::default()),
        Box::new(MemoryDraftStore::default()),
    );
    app.set_attempt_source(Box::new(SmokeAttemptSource::default()));
    ensure(
        app.fixture().repositories.len() >= 2,
        "application must retain the demo fixture",
    )?;
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
    input(&mut app, 'c');
    ensure(
        matches!(app.mode(), Mode::Edit { .. }),
        "c must open the commit draft editor",
    )?;
    for character in "fictional hjklq\nreview".chars() {
        app.handle_input(if character == '\n' {
            Input::Enter
        } else {
            Input::Character(character)
        });
    }
    ensure_contains(
        &render_frame(&mut terminal, &mut app, &mut frames)?,
        "EDIT comment",
        "commit editor frame",
    )?;
    app.handle_input(Input::Escape);
    ensure(app.draft_count() == 1, "Escape must save the commit draft")?;
    ensure_contains(
        &render_frame(&mut terminal, &mut app, &mut frames)?,
        "◆",
        "commit draft marker frame",
    )?;
    input(&mut app, 'j');
    ensure(
        app.selected(Pane::Commit) == 1,
        "moving away must select a different commit before reopening its draft",
    )?;
    input(&mut app, 'k');
    ensure(
        app.selected(Pane::Commit) == 0,
        "moving back must restore the original commit selection",
    )?;
    input(&mut app, 'c');
    ensure_contains(
        &render_frame(&mut terminal, &mut app, &mut frames)?,
        "fictional hjklq",
        "reopened commit draft frame",
    )?;
    resize(&mut terminal, 80, 24)?;
    ensure_contains(
        &render_frame(&mut terminal, &mut app, &mut frames)?,
        "EDIT comment",
        "resized commit editor frame",
    )?;
    resize(&mut terminal, FULL_WIDTH, FULL_HEIGHT)?;
    app.handle_input(Input::Cancel);
    ensure(
        app.mode() == Mode::Normal && app.draft_count() == 1,
        "Ctrl-g must leave the saved draft unchanged",
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

    app.handle_input(Input::Escape);
    ensure(
        app.focus() == Pane::File,
        "Escape must return from diff to the file pane",
    )?;
    app.handle_input(Input::Escape);
    ensure(
        app.focus() == Pane::Commit,
        "Escape must return from files to the commit pane",
    )?;
    app.handle_input(Input::Escape);
    ensure(
        app.focus() == Pane::Repository,
        "Escape must return from commits to the repository pane",
    )?;
    app.handle_input(Input::Enter);
    ensure(
        app.focus() == Pane::Commit,
        "Enter must return from repository to the commit pane",
    )?;
    app.handle_input(Input::Enter);
    ensure(
        app.focus() == Pane::File,
        "Enter must return from commit to the file pane",
    )?;
    app.handle_input(Input::Enter);
    ensure(
        app.focus() == Pane::Diff,
        "Enter must return from file to the diff pane",
    )?;
    input(&mut app, 'c');
    ensure(
        app.mode() == Mode::Normal && app.draft_count() == 1,
        "a hunk header must refuse a line draft",
    )?;
    input(&mut app, 'j');
    input(&mut app, 'c');
    ensure(
        matches!(app.mode(), Mode::Edit { .. }),
        "c must open an eligible line draft",
    )?;
    for character in "fictional q line draft".chars() {
        input(&mut app, character);
    }
    app.handle_input(Input::Escape);
    ensure(
        !app.should_quit() && app.draft_count() == 2,
        "q must insert in EDIT and Escape must save the line draft",
    )?;

    input(&mut app, 'E');
    exercise_mock_editor(&mut app)?;
    ensure(
        app.status()
            .contains("Comment draft saved from external editor"),
        "mock external-editor replacement must be saved",
    )?;
    input(&mut app, 'c');
    ensure_contains(
        &render_frame(&mut terminal, &mut app, &mut frames)?,
        EDITOR_REPLACEMENT,
        "externally replaced line draft frame",
    )?;
    app.handle_input(Input::Escape);

    let mut comments = FakeComments::default();
    comments.queue_publish_outcome(PublishOutcome::Created { id: 101 });
    comments.queue_publish_outcome(PublishOutcome::DefinitelyNotCreated(CommentFailure {
        kind: CommentFailureKind::Api,
        http_status: Some(422),
    }));
    input(&mut app, 'C');
    drain_fake_comments(&mut app, &mut comments);
    let existing_comments = render_frame(&mut terminal, &mut app, &mut frames)?;
    ensure_contains(
        &existing_comments,
        "fictional-reviewer",
        "network-free comments frame",
    )?;
    ensure_contains(
        &existing_comments,
        "fictional-line-reviewer",
        "existing line comment frame",
    )?;
    ensure_contains(
        &existing_comments,
        "src/welcome.rs:3",
        "existing line comment target frame",
    )?;
    app.handle_input(Input::Escape);
    input(&mut app, 'P');
    ensure(
        matches!(app.mode(), Mode::ConfirmPublish { .. }),
        "P must require explicit confirmation",
    )?;
    ensure_contains(
        &render_frame(&mut terminal, &mut app, &mut frames)?,
        "Press y to publish",
        "publish confirmation frame",
    )?;
    input(&mut app, 'y');
    drain_fake_comments(&mut app, &mut comments);
    ensure(
        app.draft_count() == 1 && comments.publish_count() == 1,
        "fake publish must clear only its line draft",
    )?;
    ensure_contains(
        &render_frame(&mut terminal, &mut app, &mut frames)?,
        "Comment published",
        "fake publish success frame",
    )?;
    drain_fake_comments(&mut app, &mut comments);
    input(&mut app, 'C');
    drain_fake_comments(&mut app, &mut comments);
    ensure_contains(
        &render_frame(&mut terminal, &mut app, &mut frames)?,
        EDITOR_REPLACEMENT,
        "refreshed published comment frame",
    )?;
    ensure_contains(
        &render_frame(&mut terminal, &mut app, &mut frames)?,
        "src/welcome.rs:1",
        "refreshed published line target frame",
    )?;
    app.handle_input(Input::Escape);

    app.handle_input(Input::Escape);
    ensure(
        app.focus() == Pane::File,
        "Escape must return to the file pane",
    )?;
    input(&mut app, 'P');
    ensure(
        matches!(app.mode(), Mode::ConfirmPublish { .. }),
        "the remaining commit draft must require confirmation",
    )?;
    input(&mut app, 'y');
    drain_fake_comments(&mut app, &mut comments);
    ensure(
        app.draft_count() == 1 && comments.publish_count() == 2,
        "a mocked 422 must retain the commit draft without a duplicate request",
    )?;
    ensure(
        app.status().contains("draft preserved") && app.status().contains("HTTP 422"),
        "a mocked 422 must report failure retention",
    )?;
    input(&mut app, 'c');
    ensure_contains(
        &render_frame(&mut terminal, &mut app, &mut frames)?,
        "fictional hjklq",
        "failed publish retained draft frame",
    )?;
    app.handle_input(Input::Cancel);
    app.handle_input(Input::Enter);
    ensure(
        app.focus() == Pane::Diff,
        "the keyboard flow must return to the diff after failure",
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
    ensure_contains(&tail, "launch", "diff tail frame")?;
    ensure_contains(&tail, "complete", "diff tail frame")?;
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

    let reviewed_repository = app
        .current_repository()
        .expect("fixture has the selected repository")
        .identity
        .id;
    let reviewed_sha = app
        .current_commit()
        .expect("fixture has the selected commit")
        .sha
        .clone();
    input(&mut app, 'm');
    ensure_contains(
        &render_frame(&mut terminal, &mut app, &mut frames)?,
        "✓",
        "reviewed marker frame",
    )?;
    input(&mut app, 'f');
    ensure_contains(
        &render_frame(&mut terminal, &mut app, &mut frames)?,
        "Showing remaining commits only",
        "reviewed commit remaining-filter frame",
    )?;
    input(&mut app, 'f');
    ensure_contains(
        &render_frame(&mut terminal, &mut app, &mut frames)?,
        "✓",
        "reviewed marker must survive returning to all commits",
    )?;
    ensure(
        app.is_reviewed(reviewed_repository, &reviewed_sha),
        "the same commit must remain reviewed after filter round-trip",
    )?;
    input(&mut app, 'g');
    input(&mut app, 'g');
    ensure(
        app.focus() == Pane::Commit
            && app
                .current_commit()
                .is_some_and(|commit| commit.sha == reviewed_sha),
        "returning to all commits must make the reviewed commit available to unmark",
    )?;
    input(&mut app, 'm');
    let unreviewed = render_frame(&mut terminal, &mut app, &mut frames)?;
    ensure_not_contains(&unreviewed, "✓", "unreviewed marker frame")?;
    ensure_contains(
        &unreviewed,
        "Marked commit unreviewed",
        "unreviewed status frame",
    )?;
    app.handle_input(Input::Enter);
    app.handle_input(Input::Enter);
    ensure(
        app.focus() == Pane::Diff,
        "unreviewed flow must return to the diff pane",
    )?;

    app.handle_input(Input::Escape);
    search(&mut app, "fictional-orbit-map.bin")?;
    ensure(
        app.current_file().map(|file| file.path.as_str()) == Some("assets/fictional-orbit-map.bin"),
        "file search must select the no-patch binary fixture",
    )?;
    app.handle_input(Input::Enter);
    ensure_contains(
        &render_frame(&mut terminal, &mut app, &mut frames)?,
        NO_PATCH_LABEL,
        "no-patch frame",
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
        RESPONSE_TRUNCATED_LABEL,
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

    exercise_event_loop_exits()?;

    Ok(SmokeReport { frames })
}

fn exercise_selected_day_presentation() -> io::Result<usize> {
    let now = Utc.with_ymd_and_hms(2024, 3, 10, 12, 0, 0).unwrap();
    let explicit = cli::parse_with(
        ["--date", "2024-03-10", "--timezone", "America/New_York"]
            .into_iter()
            .map(OsString::from),
        &SmokeTimezoneDetector(Ok("Etc/UTC".to_owned())),
        now,
    )
    .map_err(|error| io::Error::other(error.to_string()))?;
    let Command::Live(explicit) = explicit else {
        return Err(io::Error::other(
            "smoke expected an explicit live day selection",
        ));
    };
    let fallback = cli::parse_with(
        std::iter::empty::<OsString>(),
        &SmokeTimezoneDetector(Err("fictional detector unavailable".to_owned())),
        now,
    )
    .map_err(|error| io::Error::other(error.to_string()))?;
    let Command::Live(fallback) = fallback else {
        return Err(io::Error::other(
            "smoke expected a fallback live day selection",
        ));
    };

    let mut terminal =
        Terminal::new(TestBackend::new(FULL_WIDTH, FULL_HEIGHT)).map_err(io::Error::other)?;
    let mut frames = 0;
    let mut app = App::with_stores(
        Inbox::live(explicit),
        Box::new(MemoryReviewStore::default()),
        Box::new(MemoryDraftStore::default()),
    );
    let explicit_frame = render_frame(&mut terminal, &mut app, &mut frames)?;
    ensure_contains(
        &explicit_frame,
        "Selected day: 2024-03-10",
        "explicit selected-day presentation",
    )?;
    ensure_contains(
        &explicit_frame,
        "Timezone: America/New_York",
        "explicit timezone presentation",
    )?;

    let mut fallback_app = App::with_stores(
        Inbox::live(fallback),
        Box::new(MemoryReviewStore::default()),
        Box::new(MemoryDraftStore::default()),
    );
    let fallback_frame = render_frame(&mut terminal, &mut fallback_app, &mut frames)?;
    ensure_contains(
        &fallback_frame,
        "Selected day: 2024-03-10",
        "fallback selected-day presentation",
    )?;
    ensure_contains(
        &fallback_frame,
        "Timezone: Etc/UTC (local detection failed; UTC fallback)",
        "fallback timezone presentation",
    )?;

    Ok(frames)
}

fn exercise_event_loop_exits() -> io::Result<()> {
    exercise_event_loop_exit(
        "normal q exit",
        VecDeque::from([AppEvent::Input(Input::Character('q'))]),
    )?;
    exercise_event_loop_exit(
        "search Ctrl-c exit",
        VecDeque::from([
            AppEvent::Input(Input::Character('/')),
            AppEvent::Input(Input::Quit),
        ]),
    )
}

fn exercise_event_loop_exit(label: &str, events: VecDeque<AppEvent>) -> io::Result<()> {
    let operation_log = Arc::new(Mutex::new(Vec::new()));
    let mut app = App::with_stores(
        DemoFixture::load(),
        Box::new(MemoryReviewStore::default()),
        Box::new(MemoryDraftStore::default()),
    );
    let mut events = ScriptedEvents(events);
    let editor_log = Arc::new(Mutex::new(Vec::new()));
    let mut temp_files = MemoryEditorFiles {
        body: Arc::new(Mutex::new(String::new())),
        log: editor_log,
    };
    let mut process = NeverEditor;
    let lookup = |_name: &str| None::<OsString>;

    crate::terminal::with_terminal_session(
        RecordingTerminalOps(Arc::clone(&operation_log)),
        |guard| {
            let mut terminal = Terminal::new(TestBackend::new(FULL_WIDTH, FULL_HEIGHT))
                .map_err(io::Error::other)?;
            let mut editor =
                event::ExternalEditorSession::new(guard, &mut process, &mut temp_files, &lookup);
            event::run_with_editor(&mut terminal, &mut app, &mut events, &mut editor)?;
            ensure(
                app.should_quit(),
                &format!("{label} must quit through the event loop"),
            )
        },
    )?;
    ensure(
        *operation_log.lock().expect("smoke operation log")
            == [
                "enable_raw_mode",
                "enter_alternate_screen",
                "hide_cursor",
                "show_cursor",
                "leave_alternate_screen",
                "disable_raw_mode",
            ],
        &format!("{label} must restore the complete terminal sequence"),
    )
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

fn exercise_mock_editor(app: &mut App) -> io::Result<()> {
    let mut requests = app.take_editor_requests();
    ensure(
        requests.len() == 1,
        "E must queue exactly one external-editor request",
    )?;
    let request = requests.remove(0);
    ensure(
        request.body == "fictional q line draft",
        "external editor must receive the saved line draft",
    )?;

    let operation_log = Arc::new(Mutex::new(Vec::new()));
    let mut terminal_guard =
        TerminalGuard::acquire(RecordingTerminalOps(Arc::clone(&operation_log)))?;
    operation_log.lock().expect("smoke operation log").clear();

    let editor_body = Arc::new(Mutex::new(String::new()));
    let mut temp_files = MemoryEditorFiles {
        body: Arc::clone(&editor_body),
        log: Arc::clone(&operation_log),
    };
    let mut process = ReplacingEditor {
        body: editor_body,
        log: Arc::clone(&operation_log),
        runs: 0,
    };
    let command = EditorCommand {
        program: "nvim".into(),
        args: vec!["--clean".into()],
    };
    let result = edit_draft(
        &request.body,
        &command,
        &mut terminal_guard,
        &mut process,
        &mut temp_files,
    );

    ensure(result.resume.is_ok(), "mock terminal must be reacquired")?;
    ensure(
        result.terminal_touched,
        "mock editor must exercise terminal suspension",
    )?;
    ensure(
        result.outcome == EditorOutcome::Replaced(EDITOR_REPLACEMENT.to_owned()),
        "mock editor must replace the draft body",
    )?;
    ensure(
        process.runs == 1,
        "smoke must invoke only its in-process editor double once",
    )?;
    ensure(
        *operation_log.lock().expect("smoke operation log")
            == [
                "create",
                "show_cursor",
                "leave_alternate_screen",
                "disable_raw_mode",
                "launch",
                "read",
                "cleanup",
                "enable_raw_mode",
                "enter_alternate_screen",
                "hide_cursor",
            ],
        "terminal must restore before mock launch and reacquire afterward",
    )?;
    app.apply_editor_outcome(request, result.outcome);
    Ok(())
}

fn drain_fake_comments(app: &mut App, comments: &mut FakeComments) {
    for effect in app.take_comment_effects() {
        comments.request(effect);
    }
    while let Some(result) = comments.try_next() {
        app.apply_comment_result(result);
    }
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
