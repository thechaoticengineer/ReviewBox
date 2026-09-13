use std::collections::VecDeque;
use std::ffi::OsString;
use std::io;
use std::time::Duration;

use crossterm::event::{
    self, Event as CrosstermEvent, KeyCode, KeyEvent, KeyEventKind, KeyModifiers,
};
use ratatui::Terminal;
use ratatui::backend::Backend;
use ratatui::layout::Rect;

use crate::app::{App, CommentEffect, CommentResult, DetailEffect, DetailResult, Input};
use crate::external_editor::{
    EditorError, EditorProcess, EditorTempFiles, edit_draft, resolve_editor,
};
use crate::github::LoadEvent;
use crate::github::{ExistingComment, ExistingCommentAnchor, ExistingComments, PublishOutcome};
use crate::render;
use crate::terminal::TerminalSuspend;

const POLL_INTERVAL: Duration = Duration::from_millis(25);
const MAX_LOADER_EVENTS_PER_TICK: usize = 32;
const MAX_DETAIL_RESULTS_PER_TICK: usize = 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppEvent {
    Input(Input),
    Resize(u16, u16),
}

pub fn translate_key(key: KeyEvent) -> Option<Input> {
    if key.kind != KeyEventKind::Press {
        return None;
    }

    let control = key.modifiers.contains(KeyModifiers::CONTROL);
    let alt = key.modifiers.contains(KeyModifiers::ALT);
    let command = match (key.code, control, alt) {
        (KeyCode::Char('c'), true, _) => Input::Quit,
        (KeyCode::Char('g'), true, _) => Input::Cancel,
        (KeyCode::Char('e'), true, _) => Input::ExternalEditor,
        (KeyCode::Char('d'), true, _) => Input::HalfPageDown,
        (KeyCode::Char('u'), true, _) => Input::HalfPageUp,
        (KeyCode::Char(character), false, false) => Input::Character(character),
        (KeyCode::Enter, false, false) => Input::Enter,
        (KeyCode::Esc, false, false) => Input::Escape,
        (KeyCode::Backspace, false, false) => Input::Backspace,
        (KeyCode::Left, false, false) => Input::Left,
        (KeyCode::Right, false, false) => Input::Right,
        (KeyCode::Up, false, false) => Input::Up,
        (KeyCode::Down, false, false) => Input::Down,
        (KeyCode::Home, false, false) => Input::Home,
        (KeyCode::End, false, false) => Input::End,
        _ => Input::Unrelated,
    };
    Some(command)
}

pub trait EventSource {
    fn poll(&mut self, timeout: Duration) -> io::Result<Option<AppEvent>>;
}

pub trait LoaderEventSource {
    fn try_next(&mut self) -> Option<LoadEvent>;
    fn cancel(&mut self) {}
}

pub struct NoLoaderEvents;

impl LoaderEventSource for NoLoaderEvents {
    fn try_next(&mut self) -> Option<LoadEvent> {
        None
    }
}

pub trait DetailRequester {
    fn request(&mut self, effect: DetailEffect);
    fn cancel(&mut self, request_id: u64);
    fn try_next(&mut self) -> Option<DetailResult>;
    fn shutdown(&mut self);
}

#[derive(Default)]
pub struct NoDetails;

impl DetailRequester for NoDetails {
    fn request(&mut self, _effect: DetailEffect) {}
    fn cancel(&mut self, _request_id: u64) {}
    fn try_next(&mut self) -> Option<DetailResult> {
        None
    }
    fn shutdown(&mut self) {}
}

pub trait CommentRequester {
    fn request(&mut self, effect: CommentEffect);
    fn cancel(&mut self, request_id: u64);
    fn try_next(&mut self) -> Option<CommentResult>;
    fn shutdown(&mut self);
}

/// Network-incapable comment service for demo, smoke, and fixture runs.
#[derive(Default)]
pub struct FakeComments {
    results: VecDeque<CommentResult>,
}

impl CommentRequester for FakeComments {
    fn request(&mut self, effect: CommentEffect) {
        match effect {
            CommentEffect::Load {
                request_id,
                key,
                reconcile_target,
                marker,
                ..
            } => self.results.push_back(CommentResult {
                request_id,
                key,
                target: reconcile_target,
                outcome: crate::app::CommentResultOutcome::Loaded(Ok(ExistingComments {
                    comments: vec![ExistingComment {
                        id: 1,
                        author: Some("fictional-reviewer".to_owned()),
                        created_at: chrono::DateTime::from_timestamp(1_704_110_400, 0)
                            .expect("fixed demo timestamp"),
                        anchor: ExistingCommentAnchor::Commit,
                        body: "Fictional existing feedback for the offline demo.".to_owned(),
                    }],
                    complete: true,
                    marker_found: marker.is_some(),
                })),
            }),
            CommentEffect::Publish {
                request_id,
                key,
                target,
                ..
            } => self.results.push_back(CommentResult {
                request_id,
                key,
                target: Some(target),
                outcome: crate::app::CommentResultOutcome::Published(PublishOutcome::Created {
                    id: 1,
                }),
            }),
            CommentEffect::Cancel { .. } => {}
        }
    }
    fn cancel(&mut self, _request_id: u64) {}
    fn try_next(&mut self) -> Option<CommentResult> {
        self.results.pop_front()
    }
    fn shutdown(&mut self) {
        self.results.clear();
    }
}

pub struct CrosstermEventSource;

impl EventSource for CrosstermEventSource {
    fn poll(&mut self, timeout: Duration) -> io::Result<Option<AppEvent>> {
        if !event::poll(timeout)? {
            return Ok(None);
        }
        match event::read()? {
            CrosstermEvent::Key(key) => Ok(translate_key(key).map(AppEvent::Input)),
            CrosstermEvent::Resize(width, height) => Ok(Some(AppEvent::Resize(width, height))),
            _ => Ok(None),
        }
    }
}

pub struct ExternalEditorSession<'a> {
    terminal: &'a mut dyn TerminalSuspend,
    process: &'a mut dyn EditorProcess,
    temp_files: &'a mut dyn EditorTempFiles,
    lookup: &'a dyn Fn(&str) -> Option<OsString>,
}

impl<'a> ExternalEditorSession<'a> {
    pub fn new(
        terminal: &'a mut dyn TerminalSuspend,
        process: &'a mut dyn EditorProcess,
        temp_files: &'a mut dyn EditorTempFiles,
        lookup: &'a dyn Fn(&str) -> Option<OsString>,
    ) -> Self {
        Self {
            terminal,
            process,
            temp_files,
            lookup,
        }
    }
}

#[cfg(test)]
pub fn run<B: Backend, E: EventSource>(
    terminal: &mut Terminal<B>,
    app: &mut App,
    events: &mut E,
) -> io::Result<()>
where
    B::Error: std::error::Error + Send + Sync + 'static,
{
    let mut loader = NoLoaderEvents;
    let mut details = NoDetails;
    run_with_loader(terminal, app, events, &mut loader, &mut details)
}

#[cfg(test)]
pub fn run_with_loader<B: Backend, E: EventSource, L: LoaderEventSource, D: DetailRequester>(
    terminal: &mut Terminal<B>,
    app: &mut App,
    events: &mut E,
    loader: &mut L,
    details: &mut D,
) -> io::Result<()>
where
    B::Error: std::error::Error + Send + Sync + 'static,
{
    let mut comments = FakeComments::default();
    let result = run_loop(terminal, app, events, loader, details, &mut comments, None);
    loader.cancel();
    details.shutdown();
    comments.shutdown();
    result
}

pub fn run_with_editor<B: Backend, E: EventSource>(
    terminal: &mut Terminal<B>,
    app: &mut App,
    events: &mut E,
    editor: &mut ExternalEditorSession<'_>,
) -> io::Result<()>
where
    B::Error: std::error::Error + Send + Sync + 'static,
{
    let mut loader = NoLoaderEvents;
    let mut details = NoDetails;
    let mut comments = FakeComments::default();
    let result = run_loop(
        terminal,
        app,
        events,
        &mut loader,
        &mut details,
        &mut comments,
        Some(editor),
    );
    loader.cancel();
    details.shutdown();
    comments.shutdown();
    result
}

pub fn run_with_services_and_editor<
    B: Backend,
    E: EventSource,
    L: LoaderEventSource,
    D: DetailRequester,
    C: CommentRequester,
>(
    terminal: &mut Terminal<B>,
    app: &mut App,
    events: &mut E,
    loader: &mut L,
    details: &mut D,
    comments: &mut C,
    editor: &mut ExternalEditorSession<'_>,
) -> io::Result<()>
where
    B::Error: std::error::Error + Send + Sync + 'static,
{
    let result = run_loop(
        terminal,
        app,
        events,
        loader,
        details,
        comments,
        Some(editor),
    );
    loader.cancel();
    details.shutdown();
    comments.shutdown();
    result
}

fn run_loop<
    B: Backend,
    E: EventSource,
    L: LoaderEventSource,
    D: DetailRequester,
    C: CommentRequester,
>(
    terminal: &mut Terminal<B>,
    app: &mut App,
    events: &mut E,
    loader: &mut L,
    details: &mut D,
    comments: &mut C,
    mut editor: Option<&mut ExternalEditorSession<'_>>,
) -> io::Result<()>
where
    B::Error: std::error::Error + Send + Sync + 'static,
{
    draw(terminal, app)?;

    while !app.should_quit() {
        if let Some(event) = events.poll(POLL_INTERVAL)? {
            match event {
                AppEvent::Input(input) => app.handle_input(input),
                AppEvent::Resize(width, height) => {
                    terminal
                        .resize(Rect::new(0, 0, width, height))
                        .map_err(io::Error::other)?;
                    app.resize(width, height);
                }
            }
        }

        apply_editor_requests(terminal, app, editor.as_deref_mut())?;

        apply_detail_effects(app, details);
        apply_comment_effects(app, comments);

        if app.should_quit() {
            break;
        }

        for _ in 0..MAX_LOADER_EVENTS_PER_TICK {
            let Some(event) = loader.try_next() else {
                break;
            };
            app.apply_load_event(event);
        }
        apply_detail_effects(app, details);

        for _ in 0..MAX_DETAIL_RESULTS_PER_TICK {
            let Some(result) = details.try_next() else {
                break;
            };
            app.apply_detail_result(result);
        }
        for _ in 0..MAX_DETAIL_RESULTS_PER_TICK {
            let Some(result) = comments.try_next() else {
                break;
            };
            app.apply_comment_result(result);
        }
        apply_comment_effects(app, comments);

        draw(terminal, app)?;
    }

    Ok(())
}

fn apply_comment_effects<C: CommentRequester>(app: &mut App, comments: &mut C) {
    for effect in app.take_comment_effects() {
        match effect {
            request @ (CommentEffect::Load { .. } | CommentEffect::Publish { .. }) => {
                comments.request(request)
            }
            CommentEffect::Cancel { request_id } => comments.cancel(request_id),
        }
    }
}

fn apply_editor_requests<B: Backend>(
    terminal: &mut Terminal<B>,
    app: &mut App,
    mut editor: Option<&mut ExternalEditorSession<'_>>,
) -> io::Result<()>
where
    B::Error: std::error::Error + Send + Sync + 'static,
{
    for request in app.take_editor_requests() {
        let Some(session) = editor.as_mut() else {
            app.apply_editor_outcome(
                request,
                crate::external_editor::EditorOutcome::Failed(EditorError::NotConfigured),
            );
            continue;
        };
        let command = match resolve_editor(|name| (session.lookup)(name)) {
            Ok(command) => command,
            Err(error) => {
                app.apply_editor_outcome(
                    request,
                    crate::external_editor::EditorOutcome::Failed(error),
                );
                continue;
            }
        };
        let result = edit_draft(
            &request.body,
            &command,
            session.terminal,
            session.process,
            session.temp_files,
        );
        let outcome = result.outcome;
        if let Err(_error) = result.resume {
            app.apply_editor_outcome(request, outcome);
            return Err(io::Error::other(
                "external editor terminal restoration failed",
            ));
        }
        if result.terminal_touched {
            terminal.clear().map_err(io::Error::other)?;
            terminal.autoresize().map_err(io::Error::other)?;
        }
        app.apply_editor_outcome(request, outcome);
    }
    Ok(())
}

fn apply_detail_effects<D: DetailRequester>(app: &mut App, details: &mut D) {
    for effect in app.take_detail_effects() {
        match effect {
            request @ DetailEffect::Request { .. } => details.request(request),
            DetailEffect::Cancel { request_id } => details.cancel(request_id),
        }
    }
}

fn draw<B: Backend>(terminal: &mut Terminal<B>, app: &mut App) -> io::Result<()>
where
    B::Error: std::error::Error + Send + Sync + 'static,
{
    terminal
        .draw(|frame| {
            let area = frame.area();
            app.resize(area.width, area.height);
            render::draw(frame, app);
        })
        .map(|_| ())
        .map_err(io::Error::other)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::convert::Infallible;
    use std::path::{Path, PathBuf};
    use std::sync::Arc;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicBool, Ordering};

    use crate::comment_draft::{
        CommentDraft, CommentDrafts, CommentTarget, DraftStateError, DraftStore,
    };
    use crate::external_editor::EditorCommand;
    use crate::fixture::DemoFixture;
    use crate::github::{
        FailureCategory, FailureScope, LoadFailure, LoadProgress, LoadStatus, LoadedRepository,
        RepositoryCoverage,
    };
    use crate::inbox::{
        ChildPane, Commit, CommitDetail, FileChange, FileStatus, GitHubAuthor, Inbox, Repository,
        RepositoryIdentity,
    };
    use crate::review_state::MemoryReviewStore;
    use chrono::{TimeZone, Utc};
    use ratatui::backend::{Backend, ClearType, TestBackend, WindowSize};
    use ratatui::buffer::Cell;
    use ratatui::layout::{Position, Size};
    use ratatui::{TerminalOptions, Viewport};

    struct RecordingBackend {
        inner: TestBackend,
        log: Arc<Mutex<Vec<&'static str>>>,
    }

    impl RecordingBackend {
        fn new(log: Arc<Mutex<Vec<&'static str>>>) -> Self {
            Self {
                inner: TestBackend::new(100, 24),
                log,
            }
        }
    }

    impl Backend for RecordingBackend {
        type Error = Infallible;

        fn draw<'a, I>(&mut self, content: I) -> Result<(), Self::Error>
        where
            I: Iterator<Item = (u16, u16, &'a Cell)>,
        {
            self.log.lock().unwrap().push("redraw");
            self.inner.draw(content)
        }

        fn hide_cursor(&mut self) -> Result<(), Self::Error> {
            self.inner.hide_cursor()
        }

        fn show_cursor(&mut self) -> Result<(), Self::Error> {
            self.inner.show_cursor()
        }

        fn get_cursor_position(&mut self) -> Result<Position, Self::Error> {
            self.inner.get_cursor_position()
        }

        fn set_cursor_position<P: Into<Position>>(
            &mut self,
            position: P,
        ) -> Result<(), Self::Error> {
            self.inner.set_cursor_position(position)
        }

        fn clear(&mut self) -> Result<(), Self::Error> {
            self.log.lock().unwrap().push("clear");
            self.inner.clear()
        }

        fn clear_region(&mut self, clear_type: ClearType) -> Result<(), Self::Error> {
            self.log.lock().unwrap().push("clear");
            self.inner.clear_region(clear_type)
        }

        fn size(&self) -> Result<Size, Self::Error> {
            self.inner.size()
        }

        fn window_size(&mut self) -> Result<WindowSize, Self::Error> {
            self.inner.window_size()
        }

        fn flush(&mut self) -> Result<(), Self::Error> {
            self.inner.flush()
        }
    }

    struct RecordingSuspend {
        log: Arc<Mutex<Vec<&'static str>>>,
        suspend_failure: bool,
        resume_failure: bool,
    }

    impl TerminalSuspend for RecordingSuspend {
        fn suspend(&mut self) -> io::Result<()> {
            let mut log = self.log.lock().unwrap();
            log.extend(["show_cursor", "leave_alternate_screen", "disable_raw_mode"]);
            if self.suspend_failure {
                Err(io::Error::other("injected suspend failure"))
            } else {
                Ok(())
            }
        }

        fn resume(&mut self) -> io::Result<()> {
            let mut log = self.log.lock().unwrap();
            log.push("enable_raw_mode");
            if self.resume_failure {
                return Err(io::Error::other("injected resume failure"));
            }
            log.extend(["enter_alternate_screen", "hide_cursor"]);
            Ok(())
        }
    }

    struct ScriptedEditorProcess {
        log: Arc<Mutex<Vec<&'static str>>>,
        result: Result<bool, EditorError>,
    }

    impl EditorProcess for ScriptedEditorProcess {
        fn run(&mut self, command: &EditorCommand, path: &Path) -> Result<bool, EditorError> {
            assert_eq!(command.program, "nvim");
            assert_eq!(command.args, ["-f"]);
            assert_eq!(path, Path::new("/private/draft.md"));
            self.log.lock().unwrap().push("launch");
            self.result
        }
    }

    struct ScriptedTempFiles {
        create: Result<PathBuf, EditorError>,
        read: Result<String, EditorError>,
        cleaned: bool,
    }

    impl EditorTempFiles for ScriptedTempFiles {
        fn create(&mut self, _body: &str) -> Result<PathBuf, EditorError> {
            self.create.clone()
        }

        fn read(&mut self, _path: &Path) -> Result<String, EditorError> {
            self.read.clone()
        }

        fn cleanup(&mut self, _path: &Path) -> Result<(), EditorError> {
            self.cleaned = true;
            Ok(())
        }
    }

    fn queue_normal_editor(app: &mut App) {
        app.handle_input(Input::Character('l'));
        app.handle_input(Input::Character('E'));
    }

    fn run_editor_request(
        app: &mut App,
        process_result: Result<bool, EditorError>,
        read_result: Result<String, EditorError>,
        suspend_failure: bool,
        resume_failure: bool,
    ) -> (io::Result<()>, Vec<&'static str>, bool) {
        let log = Arc::new(Mutex::new(Vec::new()));
        let mut terminal = Terminal::new(RecordingBackend::new(Arc::clone(&log))).unwrap();
        let mut terminal_state = RecordingSuspend {
            log: Arc::clone(&log),
            suspend_failure,
            resume_failure,
        };
        let mut process = ScriptedEditorProcess {
            log: Arc::clone(&log),
            result: process_result,
        };
        let mut temp_files = ScriptedTempFiles {
            create: Ok(PathBuf::from("/private/draft.md")),
            read: read_result,
            cleaned: false,
        };
        let lookup = |name: &str| (name == "VISUAL").then(|| OsString::from("nvim -f"));
        let mut session =
            ExternalEditorSession::new(&mut terminal_state, &mut process, &mut temp_files, &lookup);
        let result = apply_editor_requests(&mut terminal, app, Some(&mut session));
        if result.is_ok() {
            draw(&mut terminal, app).unwrap();
        }
        let calls = log.lock().unwrap().clone();
        (result, calls, temp_files.cleaned)
    }

    struct Events(Vec<io::Result<AppEvent>>);

    impl EventSource for Events {
        fn poll(&mut self, _timeout: Duration) -> io::Result<Option<AppEvent>> {
            self.0.remove(0).map(Some)
        }
    }

    struct ScriptedEvents(VecDeque<io::Result<Option<AppEvent>>>);

    impl EventSource for ScriptedEvents {
        fn poll(&mut self, _timeout: Duration) -> io::Result<Option<AppEvent>> {
            self.0.pop_front().expect("scripted event exhausted")
        }
    }

    struct ScriptedLoader {
        events: VecDeque<Option<LoadEvent>>,
        cancelled: Arc<AtomicBool>,
    }

    impl LoaderEventSource for ScriptedLoader {
        fn try_next(&mut self) -> Option<LoadEvent> {
            self.events.pop_front().flatten()
        }

        fn cancel(&mut self) {
            self.cancelled.store(true, Ordering::Release);
        }
    }

    #[derive(Default)]
    struct ScriptedDetails {
        requests: Vec<u64>,
        cancellations: Vec<u64>,
        results: VecDeque<DetailResult>,
        auto_succeed: bool,
        shutdown: bool,
    }

    impl DetailRequester for ScriptedDetails {
        fn request(&mut self, effect: DetailEffect) {
            let DetailEffect::Request {
                request_id, key, ..
            } = effect
            else {
                return;
            };
            self.requests.push(request_id);
            if self.auto_succeed {
                self.results.push_back(DetailResult {
                    request_id,
                    key,
                    outcome: Ok(CommitDetail {
                        files: vec![FileChange {
                            path: "src/live.rs".to_owned(),
                            api_path_is_commentable: true,
                            previous_path: None,
                            status: FileStatus::Modified,
                            additions: 1,
                            deletions: 0,
                            changes: 1,
                            patch: crate::github::parse_patch_text("@@ -1 +1 @@\n+live details"),
                        }],
                        omitted_files: 0,
                        more_files_available: false,
                    }),
                });
            }
        }

        fn cancel(&mut self, request_id: u64) {
            self.cancellations.push(request_id);
        }

        fn try_next(&mut self) -> Option<DetailResult> {
            self.results.pop_front()
        }

        fn shutdown(&mut self) {
            self.shutdown = true;
        }
    }

    fn live_app() -> App {
        let selection = crate::day::select_day(
            crate::day::parse_date("2024-01-15").unwrap(),
            crate::day::parse_timezone("Europe/Warsaw").unwrap(),
            crate::day::TimezoneSource::Explicit,
        )
        .unwrap();
        App::new(Inbox::live(selection))
    }

    fn loaded(name: &str, sha: &str, subject: &str) -> LoadedRepository {
        let (owner, repository_name) = name.split_once('/').unwrap();
        LoadedRepository {
            repository: Repository {
                identity: RepositoryIdentity {
                    id: name.bytes().map(u64::from).sum(),
                    owner: owner.to_owned(),
                    name: repository_name.to_owned(),
                },
                commits: vec![Commit {
                    sha: sha.to_owned(),
                    subject: subject.to_owned(),
                    author: GitHubAuthor {
                        login: "fixture-user".to_owned(),
                    },
                    authored_at: Utc.with_ymd_and_hms(2024, 1, 15, 12, 0, 0).unwrap(),
                    files: ChildPane::Unavailable,
                }],
            },
            branch_count: 1,
            coverage: RepositoryCoverage::Complete,
        }
    }

    fn buffer_text(terminal: &Terminal<TestBackend>) -> String {
        terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect()
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn translates_printable_and_editing_keys_without_assigning_a_mode() {
        for (key, expected) in [
            (key(KeyCode::Char('h')), Input::Character('h')),
            (key(KeyCode::Char('j')), Input::Character('j')),
            (key(KeyCode::Char('/')), Input::Character('/')),
            (key(KeyCode::Char('?')), Input::Character('?')),
            (
                KeyEvent::new(KeyCode::Char('G'), KeyModifiers::SHIFT),
                Input::Character('G'),
            ),
            (key(KeyCode::Enter), Input::Enter),
            (key(KeyCode::Esc), Input::Escape),
            (key(KeyCode::Backspace), Input::Backspace),
            (key(KeyCode::Left), Input::Left),
            (key(KeyCode::Right), Input::Right),
            (key(KeyCode::Up), Input::Up),
            (key(KeyCode::Down), Input::Down),
            (key(KeyCode::Home), Input::Home),
            (key(KeyCode::End), Input::End),
        ] {
            assert_eq!(translate_key(key), Some(expected));
        }
    }

    #[test]
    fn translates_control_movement_and_quit_keys() {
        for (character, expected) in [
            ('d', Input::HalfPageDown),
            ('u', Input::HalfPageUp),
            ('c', Input::Quit),
            ('g', Input::Cancel),
            ('e', Input::ExternalEditor),
        ] {
            let key = KeyEvent::new(KeyCode::Char(character), KeyModifiers::CONTROL);
            assert_eq!(translate_key(key), Some(expected));
        }
    }

    fn expected_editor_cycle() -> Vec<&'static str> {
        vec![
            "show_cursor",
            "leave_alternate_screen",
            "disable_raw_mode",
            "launch",
            "enable_raw_mode",
            "enter_alternate_screen",
            "hide_cursor",
            "clear",
            "redraw",
        ]
    }

    #[test]
    fn editor_terminal_cycle_is_exact_for_success_exit_and_read_failures() {
        for (process, read, expected_status) in [
            (Ok(true), Ok("replacement".to_owned()), "saved"),
            (Ok(false), Ok("ignored".to_owned()), "unchanged"),
            (
                Err(EditorError::Launch),
                Ok("ignored".to_owned()),
                "could not be run",
            ),
            (Ok(true), Err(EditorError::Read), "could not be read"),
            (Ok(true), Err(EditorError::Decode), "valid UTF-8"),
            (Ok(true), Err(EditorError::TooLarge), "draft limit"),
        ] {
            let mut app = App::new(DemoFixture::load());
            queue_normal_editor(&mut app);
            let (result, calls, cleaned) =
                run_editor_request(&mut app, process, read, false, false);

            result.unwrap();
            assert_eq!(calls, expected_editor_cycle());
            assert!(cleaned);
            assert!(app.status().contains(expected_status));
            assert_eq!(app.mode(), crate::app::Mode::Normal);
            app.handle_input(Input::Character('q'));
            assert!(app.should_quit(), "reacquired TUI must remain usable");
        }
    }

    #[test]
    fn suspend_failure_skips_launch_but_reacquires_clears_and_redraws() {
        let mut app = App::new(DemoFixture::load());
        queue_normal_editor(&mut app);
        let (result, calls, cleaned) =
            run_editor_request(&mut app, Ok(true), Ok("ignored".to_owned()), true, false);

        result.unwrap();
        assert_eq!(
            calls,
            [
                "show_cursor",
                "leave_alternate_screen",
                "disable_raw_mode",
                "enable_raw_mode",
                "enter_alternate_screen",
                "hide_cursor",
                "clear",
                "redraw",
            ]
        );
        assert!(cleaned);
        assert!(app.status().contains("terminal could not switch"));
    }

    #[test]
    fn configuration_and_temp_failures_do_not_touch_the_terminal() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let mut backend_terminal = Terminal::new(RecordingBackend::new(Arc::clone(&log))).unwrap();
        let mut terminal_state = RecordingSuspend {
            log: Arc::clone(&log),
            suspend_failure: false,
            resume_failure: false,
        };
        let mut process = ScriptedEditorProcess {
            log: Arc::clone(&log),
            result: Ok(true),
        };
        let mut temp_files = ScriptedTempFiles {
            create: Ok(PathBuf::from("/private/draft.md")),
            read: Ok("changed".to_owned()),
            cleaned: false,
        };

        let mut app = App::new(DemoFixture::load());
        queue_normal_editor(&mut app);
        let missing = |_name: &str| None;
        let mut session = ExternalEditorSession::new(
            &mut terminal_state,
            &mut process,
            &mut temp_files,
            &missing,
        );
        apply_editor_requests(&mut backend_terminal, &mut app, Some(&mut session)).unwrap();
        assert!(log.lock().unwrap().is_empty());
        assert!(app.status().contains("no external editor"));

        queue_normal_editor(&mut app);
        temp_files.create = Err(EditorError::TempFile);
        let configured = |name: &str| (name == "EDITOR").then(|| OsString::from("nvim"));
        let mut session = ExternalEditorSession::new(
            &mut terminal_state,
            &mut process,
            &mut temp_files,
            &configured,
        );
        apply_editor_requests(&mut backend_terminal, &mut app, Some(&mut session)).unwrap();
        assert!(log.lock().unwrap().is_empty());
        assert!(app.status().contains("private editor file"));
    }

    #[test]
    fn resume_failure_returns_a_sanitized_error_after_saving_read_content() {
        let mut app = App::new(DemoFixture::load());
        queue_normal_editor(&mut app);
        let (result, calls, cleaned) = run_editor_request(
            &mut app,
            Ok(true),
            Ok("saved despite resume failure".to_owned()),
            false,
            true,
        );

        assert_eq!(
            result.unwrap_err().to_string(),
            "external editor terminal restoration failed"
        );
        assert_eq!(
            calls,
            [
                "show_cursor",
                "leave_alternate_screen",
                "disable_raw_mode",
                "launch",
                "enable_raw_mode",
            ]
        );
        assert!(cleaned);
        assert_eq!(app.draft_count(), 1);
        assert!(!app.status().contains("saved despite resume failure"));
    }

    #[derive(Debug)]
    struct EventFailingDraftStore;

    impl DraftStore for EventFailingDraftStore {
        fn load(&self) -> Result<CommentDrafts, DraftStateError> {
            Ok(CommentDrafts::default())
        }

        fn save(&self, _draft: &CommentDraft) -> Result<CommentDrafts, DraftStateError> {
            Err(DraftStateError::Write(io::ErrorKind::PermissionDenied))
        }

        fn delete(&self, _target: &CommentTarget) -> Result<CommentDrafts, DraftStateError> {
            Err(DraftStateError::Write(io::ErrorKind::PermissionDenied))
        }
    }

    #[test]
    fn external_save_failure_still_reacquires_and_redraws_with_text_recoverable() {
        let mut app = App::with_stores(
            DemoFixture::load(),
            Box::new(MemoryReviewStore::default()),
            Box::new(EventFailingDraftStore),
        );
        app.handle_input(Input::Character('l'));
        app.handle_input(Input::Character('c'));
        app.handle_input(Input::Character('x'));
        app.handle_input(Input::ExternalEditor);

        let (result, calls, cleaned) = run_editor_request(
            &mut app,
            Ok(true),
            Ok("recoverable replacement".to_owned()),
            false,
            false,
        );

        result.unwrap();
        assert_eq!(calls, expected_editor_cycle());
        assert!(cleaned);
        assert_eq!(app.edit_buffer().unwrap().text(), "recoverable replacement");
        assert!(app.status().contains("not saved"));
    }

    #[test]
    fn printable_press_is_preserved_but_repeat_and_release_are_ignored() {
        assert_eq!(
            translate_key(key(KeyCode::Char('x'))),
            Some(Input::Character('x'))
        );
        for kind in [KeyEventKind::Repeat, KeyEventKind::Release] {
            let key = KeyEvent::new_with_kind(KeyCode::Char('j'), KeyModifiers::NONE, kind);
            assert_eq!(translate_key(key), None);
        }
    }

    #[test]
    fn resize_redraws_and_quit_exits() {
        let backend = TestBackend::new(80, 24);
        let options = TerminalOptions {
            viewport: Viewport::Fixed(Rect::new(0, 0, 80, 24)),
        };
        let mut terminal = Terminal::with_options(backend, options).expect("test terminal");
        let mut app = App::new(DemoFixture::load());
        let mut events = Events(vec![
            Ok(AppEvent::Resize(40, 10)),
            Ok(AppEvent::Input(Input::Character('q'))),
        ]);

        run(&mut terminal, &mut app, &mut events).expect("event loop succeeds");

        assert!(app.should_quit());
        assert_eq!((app.terminal_width, app.terminal_height), (40, 10));
    }

    #[test]
    fn inputs_reach_the_modal_reducer() {
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).expect("test terminal");
        let mut app = App::new(DemoFixture::load());
        let mut events = Events(vec![
            Ok(AppEvent::Input(Input::Character('j'))),
            Ok(AppEvent::Input(Input::Character('l'))),
            Ok(AppEvent::Input(Input::Character('q'))),
        ]);

        run(&mut terminal, &mut app, &mut events).expect("event loop succeeds");

        assert_eq!(app.selected(crate::app::Pane::Repository), 1);
        assert_eq!(app.focus(), crate::app::Pane::Commit);
    }

    #[test]
    fn event_source_errors_are_propagated() {
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).expect("test terminal");
        let mut app = App::new(DemoFixture::load());
        let mut events = Events(vec![Err(io::Error::other("forced event failure"))]);

        let error = run(&mut terminal, &mut app, &mut events).expect_err("error propagates");

        assert_eq!(error.to_string(), "forced event failure");
    }

    #[test]
    fn live_loading_renders_before_waiting_for_terminal_input() {
        let backend = TestBackend::new(100, 24);
        let mut terminal = Terminal::new(backend).expect("test terminal");
        let mut app = live_app();
        let mut events = Events(vec![Err(io::Error::other("stop after first frame"))]);

        let error = run(&mut terminal, &mut app, &mut events).expect_err("script stops loop");

        assert_eq!(error.to_string(), "stop after first frame");
        let output = buffer_text(&terminal);
        assert!(output.contains("Selected day: 2024-01-15"));
        assert!(output.contains("checking gh authentication"));
    }

    #[test]
    fn loader_progress_is_redrawn_in_bounded_batches() {
        let backend = TestBackend::new(100, 24);
        let mut terminal = Terminal::new(backend).expect("test terminal");
        let mut app = live_app();
        let cancelled = Arc::new(AtomicBool::new(false));
        let mut loader = ScriptedLoader {
            events: (1..=40)
                .map(|page| {
                    Some(LoadEvent::DiscoveryPage {
                        page,
                        owned_repositories: page,
                    })
                })
                .collect(),
            cancelled: Arc::clone(&cancelled),
        };
        let mut events = ScriptedEvents(VecDeque::from([
            Ok(None),
            Err(io::Error::other("stop after progress frame")),
        ]));
        let mut details = NoDetails;

        let error = run_with_loader(
            &mut terminal,
            &mut app,
            &mut events,
            &mut loader,
            &mut details,
        )
        .expect_err("script stops loop");

        assert_eq!(error.to_string(), "stop after progress frame");
        assert!(cancelled.load(Ordering::Acquire));
        assert_eq!(loader.events.len(), 8);
        let output = buffer_text(&terminal);
        assert!(output.contains("page 32"));
        assert!(!output.contains("page 40"));
    }

    #[test]
    fn polling_interleaves_progress_navigation_resize_and_partial_results() {
        let backend = TestBackend::new(100, 24);
        let options = TerminalOptions {
            viewport: Viewport::Fixed(Rect::new(0, 0, 100, 24)),
        };
        let mut terminal = Terminal::with_options(backend, options).expect("test terminal");
        let mut app = live_app();
        let cancelled = Arc::new(AtomicBool::new(false));
        let permission = LoadFailure {
            category: FailureCategory::PermissionOrNotFound,
            scope: FailureScope::Branch {
                repository_index: 0,
                branch_index: 1,
            },
            http_status: Some(403),
        };
        let first = loaded("fixture/alpha", "aaaaaaaa", "first partial");
        let second = loaded("fixture/beta", "bbbbbbbb", "browsable partial");
        let mut updated_first = first.clone();
        updated_first.repository.commits.push(Commit {
            sha: "cccccccc".to_owned(),
            subject: "later update".to_owned(),
            author: GitHubAuthor {
                login: "fixture-user".to_owned(),
            },
            authored_at: Utc.with_ymd_and_hms(2024, 1, 15, 13, 0, 0).unwrap(),
            files: ChildPane::Unavailable,
        });
        let mut loader = ScriptedLoader {
            events: VecDeque::from([
                Some(LoadEvent::DiscoveryPage {
                    page: 1,
                    owned_repositories: 2,
                }),
                None,
                Some(LoadEvent::RepositoriesDiscovered { total: 2 }),
                Some(LoadEvent::RepositorySnapshot {
                    repository_index: 0,
                    repository: first,
                }),
                None,
                Some(LoadEvent::RepositorySnapshot {
                    repository_index: 1,
                    repository: second,
                }),
                None,
                Some(LoadEvent::RepositorySnapshot {
                    repository_index: 0,
                    repository: updated_first,
                }),
                None,
                Some(LoadEvent::Failure(permission)),
                Some(LoadEvent::Finished {
                    status: LoadStatus::Incomplete,
                    progress: LoadProgress {
                        repositories_discovered: 2,
                        repositories_processed: 2,
                        branches_discovered: 3,
                        branches_processed: 3,
                        commits_loaded: 3,
                    },
                }),
                None,
            ]),
            cancelled: Arc::clone(&cancelled),
        };
        let mut events = ScriptedEvents(VecDeque::from([
            Ok(None),
            Ok(None),
            Ok(None),
            Ok(Some(AppEvent::Input(Input::Character('j')))),
            Ok(Some(AppEvent::Input(Input::Character('l')))),
            Ok(Some(AppEvent::Resize(80, 20))),
            Ok(Some(AppEvent::Input(Input::Character('q')))),
        ]));
        let mut details = NoDetails;

        run_with_loader(
            &mut terminal,
            &mut app,
            &mut events,
            &mut loader,
            &mut details,
        )
        .expect("event loop succeeds");

        assert!(cancelled.load(Ordering::Acquire));
        assert_eq!((app.terminal_width, app.terminal_height), (80, 20));
        assert_eq!(app.selected(crate::app::Pane::Repository), 1);
        assert_eq!(app.focus(), crate::app::Pane::Commit);
        assert_eq!(
            app.current_commit().map(|commit| commit.sha.as_str()),
            Some("bbbbbbbb")
        );
        let output = buffer_text(&terminal);
        assert!(output.contains("INCOMPLETE RESULTS"));
        assert!(output.contains("HTTP 403"));
        assert!(output.contains("browsable partial"));
        assert!(!output.contains("COMPLETE — daily inbox loaded"));
    }

    #[test]
    fn event_loop_dispatches_one_detail_request_and_renders_the_result() {
        let backend = TestBackend::new(100, 24);
        let mut terminal = Terminal::new(backend).expect("test terminal");
        let mut app = live_app();
        let cancelled = Arc::new(AtomicBool::new(false));
        let mut loader = ScriptedLoader {
            events: VecDeque::from([
                Some(LoadEvent::RepositorySnapshot {
                    repository_index: 0,
                    repository: loaded(
                        "fixture/details",
                        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                        "responsive details",
                    ),
                }),
                None,
            ]),
            cancelled: Arc::clone(&cancelled),
        };
        let mut details = ScriptedDetails {
            auto_succeed: true,
            ..ScriptedDetails::default()
        };
        let mut events = ScriptedEvents(VecDeque::from([
            Ok(None),
            Ok(Some(AppEvent::Input(Input::Character('l')))),
            Ok(Some(AppEvent::Input(Input::Enter))),
            Ok(Some(AppEvent::Input(Input::Escape))),
            Ok(Some(AppEvent::Input(Input::Enter))),
            Ok(Some(AppEvent::Input(Input::Character('q')))),
        ]));

        run_with_loader(
            &mut terminal,
            &mut app,
            &mut events,
            &mut loader,
            &mut details,
        )
        .expect("detail workflow succeeds");

        assert_eq!(
            details.requests.len(),
            1,
            "cached re-open must not duplicate"
        );
        assert!(details.cancellations.is_empty());
        assert!(details.shutdown);
        assert!(cancelled.load(Ordering::Acquire));
        assert_eq!(
            app.current_file().map(|file| file.path.as_str()),
            Some("src/live.rs")
        );
        assert!(buffer_text(&terminal).contains("src/live.rs"));
    }

    #[test]
    fn quitting_cancels_and_shuts_down_active_detail_work() {
        let backend = TestBackend::new(100, 24);
        let mut terminal = Terminal::new(backend).expect("test terminal");
        let mut app = live_app();
        let cancelled = Arc::new(AtomicBool::new(false));
        let mut loader = ScriptedLoader {
            events: VecDeque::from([
                Some(LoadEvent::RepositorySnapshot {
                    repository_index: 0,
                    repository: loaded(
                        "fixture/cancel",
                        "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
                        "cancel details",
                    ),
                }),
                None,
            ]),
            cancelled: Arc::clone(&cancelled),
        };
        let mut details = ScriptedDetails::default();
        let mut events = ScriptedEvents(VecDeque::from([
            Ok(None),
            Ok(Some(AppEvent::Input(Input::Character('l')))),
            Ok(Some(AppEvent::Input(Input::Enter))),
            Ok(Some(AppEvent::Input(Input::Character('q')))),
        ]));

        run_with_loader(
            &mut terminal,
            &mut app,
            &mut events,
            &mut loader,
            &mut details,
        )
        .expect("quit succeeds");

        assert_eq!(details.requests.len(), 1);
        assert_eq!(details.cancellations, details.requests);
        assert!(details.shutdown);
        assert!(cancelled.load(Ordering::Acquire));
    }

    #[derive(Default)]
    struct RecordingComments {
        requests: Vec<&'static str>,
        results: VecDeque<CommentResult>,
        shutdown: bool,
    }

    impl CommentRequester for RecordingComments {
        fn request(&mut self, effect: CommentEffect) {
            match effect {
                CommentEffect::Load {
                    request_id,
                    key,
                    reconcile_target,
                    ..
                } => {
                    self.requests.push("GET");
                    self.results.push_back(CommentResult {
                        request_id,
                        key,
                        target: reconcile_target,
                        outcome: crate::app::CommentResultOutcome::Loaded(Ok(ExistingComments {
                            comments: Vec::new(),
                            complete: true,
                            marker_found: false,
                        })),
                    });
                }
                CommentEffect::Publish { .. } => self.requests.push("POST"),
                CommentEffect::Cancel { .. } => {}
            }
        }
        fn cancel(&mut self, _request_id: u64) {}
        fn try_next(&mut self) -> Option<CommentResult> {
            self.results.pop_front()
        }
        fn shutdown(&mut self) {
            self.shutdown = true;
        }
    }

    #[test]
    fn event_loop_dispatches_comment_loading_through_a_network_incapable_fake() {
        let backend = TestBackend::new(100, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut app = App::new(DemoFixture::load());
        let mut events = ScriptedEvents(VecDeque::from([
            Ok(Some(AppEvent::Input(Input::Character('l')))),
            Ok(Some(AppEvent::Input(Input::Character('C')))),
            Ok(None),
            Ok(Some(AppEvent::Input(Input::Quit))),
        ]));
        let mut loader = NoLoaderEvents;
        let mut details = NoDetails;
        let mut comments = RecordingComments::default();
        run_loop(
            &mut terminal,
            &mut app,
            &mut events,
            &mut loader,
            &mut details,
            &mut comments,
            None,
        )
        .unwrap();
        comments.shutdown();
        assert_eq!(comments.requests, ["GET"]);
        assert!(comments.shutdown);
        assert!(matches!(
            app.current_comments(),
            Some(crate::app::CommentListState::Loaded(_))
        ));
    }
}
