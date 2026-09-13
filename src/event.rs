use std::io;
use std::time::Duration;

use crossterm::event::{
    self, Event as CrosstermEvent, KeyCode, KeyEvent, KeyEventKind, KeyModifiers,
};
use ratatui::Terminal;
use ratatui::backend::Backend;
use ratatui::layout::Rect;

use crate::app::{App, Input};
use crate::github::LoadEvent;
use crate::render;

const POLL_INTERVAL: Duration = Duration::from_millis(25);
const MAX_LOADER_EVENTS_PER_TICK: usize = 32;

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
        (KeyCode::Char('d'), true, _) => Input::HalfPageDown,
        (KeyCode::Char('u'), true, _) => Input::HalfPageUp,
        (KeyCode::Char(character), false, false) => Input::Character(character),
        (KeyCode::Enter, false, false) => Input::Enter,
        (KeyCode::Esc, false, false) => Input::Escape,
        (KeyCode::Backspace, false, false) => Input::Backspace,
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

pub fn run<B: Backend, E: EventSource>(
    terminal: &mut Terminal<B>,
    app: &mut App,
    events: &mut E,
) -> io::Result<()>
where
    B::Error: std::error::Error + Send + Sync + 'static,
{
    let mut loader = NoLoaderEvents;
    run_with_loader(terminal, app, events, &mut loader)
}

pub fn run_with_loader<B: Backend, E: EventSource, L: LoaderEventSource>(
    terminal: &mut Terminal<B>,
    app: &mut App,
    events: &mut E,
    loader: &mut L,
) -> io::Result<()>
where
    B::Error: std::error::Error + Send + Sync + 'static,
{
    let result = run_loop(terminal, app, events, loader);
    loader.cancel();
    result
}

fn run_loop<B: Backend, E: EventSource, L: LoaderEventSource>(
    terminal: &mut Terminal<B>,
    app: &mut App,
    events: &mut E,
    loader: &mut L,
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

        if app.should_quit() {
            break;
        }

        for _ in 0..MAX_LOADER_EVENTS_PER_TICK {
            let Some(event) = loader.try_next() else {
                break;
            };
            app.apply_load_event(event);
        }

        draw(terminal, app)?;
    }

    Ok(())
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
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    use crate::fixture::DemoFixture;
    use crate::github::{
        FailureCategory, FailureScope, LoadFailure, LoadProgress, LoadStatus, LoadedRepository,
        RepositoryCoverage,
    };
    use crate::inbox::{ChildPane, Commit, GitHubAuthor, Inbox, Repository, RepositoryIdentity};
    use chrono::{TimeZone, Utc};
    use ratatui::backend::TestBackend;
    use ratatui::{TerminalOptions, Viewport};

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
        ] {
            let key = KeyEvent::new(KeyCode::Char(character), KeyModifiers::CONTROL);
            assert_eq!(translate_key(key), Some(expected));
        }
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

        let error = run_with_loader(&mut terminal, &mut app, &mut events, &mut loader)
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

        run_with_loader(&mut terminal, &mut app, &mut events, &mut loader)
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
}
