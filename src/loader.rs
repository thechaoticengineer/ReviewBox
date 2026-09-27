use std::collections::HashMap;
use std::sync::mpsc::{Receiver, SyncSender, TrySendError, sync_channel};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crate::day::DaySelection;
use crate::event::LoaderEventSource;
use crate::github::{
    CancellationToken, CommandRunner, FailureCategory, FailureScope, GitHubLoader, HistoryCoverage,
    HistoryDiscovery, HistoryEvent, HistoryPass, LoadEvent, LoadFailure, ProcessRunner,
    RepositoryHistory,
};
use crate::inbox::{Repository, RepositoryIdentity};

const EVENT_CHANNEL_CAPACITY: usize = 64;
const COMMAND_CHANNEL_CAPACITY: usize = 8;

/// Generation of the initial backlog batch. Later history operations use
/// larger generations allocated by the application.
pub const INITIAL_GENERATION: u64 = 1;

pub struct LoaderSession {
    receiver: Option<Receiver<LoadEvent>>,
    cancellation: CancellationToken,
    worker: Option<JoinHandle<()>>,
}

impl LoaderSession {
    pub fn start(selection: DaySelection) -> Self {
        let (sender, receiver) = sync_channel(EVENT_CHANNEL_CAPACITY);
        let cancellation = CancellationToken::default();
        let worker_cancellation = cancellation.clone();
        let worker = thread::spawn(move || {
            let loader = GitHubLoader::new(CommandRunner);
            let event_cancellation = worker_cancellation.clone();
            let _ = loader.load_cancellable(&selection, &worker_cancellation, |event| {
                send_event(&sender, &event_cancellation, event);
            });
        });

        Self {
            receiver: Some(receiver),
            cancellation,
            worker: Some(worker),
        }
    }

    pub fn shutdown(&mut self) {
        self.cancellation.cancel();
        self.receiver.take();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

impl LoaderEventSource for LoaderSession {
    fn try_next(&mut self) -> Option<LoadEvent> {
        self.receiver.as_ref()?.try_recv().ok()
    }

    fn cancel(&mut self) {
        self.cancellation.cancel();
    }
}

impl Drop for LoaderSession {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn send_event(
    sender: &SyncSender<LoadEvent>,
    cancellation: &CancellationToken,
    mut event: LoadEvent,
) {
    loop {
        if cancellation.is_cancelled() {
            return;
        }
        match sender.try_send(event) {
            Ok(()) | Err(TrySendError::Disconnected(_)) => return,
            Err(TrySendError::Full(returned)) => {
                event = returned;
                thread::sleep(Duration::from_millis(5));
            }
        }
    }
}

/// Source of undated, incrementally paged commit history. The GitHub loader
/// and the fictional demo pager implement the same interface, so both run
/// through the same backlog worker.
pub trait HistoryPager {
    fn discover(
        &mut self,
        cancellation: &CancellationToken,
        emit: &mut dyn FnMut(HistoryEvent),
    ) -> Result<HistoryDiscovery, LoadFailure>;

    fn fetch_next_pages(
        &mut self,
        history: &mut RepositoryHistory,
        login: &str,
        cancellation: &CancellationToken,
        emit: &mut dyn FnMut(HistoryEvent),
    ) -> Result<HistoryPass, LoadFailure>;
}

impl<R: ProcessRunner> HistoryPager for GitHubLoader<R> {
    fn discover(
        &mut self,
        cancellation: &CancellationToken,
        emit: &mut dyn FnMut(HistoryEvent),
    ) -> Result<HistoryDiscovery, LoadFailure> {
        self.discover_history(cancellation, emit)
    }

    fn fetch_next_pages(
        &mut self,
        history: &mut RepositoryHistory,
        login: &str,
        cancellation: &CancellationToken,
        emit: &mut dyn FnMut(HistoryEvent),
    ) -> Result<HistoryPass, LoadFailure> {
        GitHubLoader::fetch_next_pages(self, history, login, cancellation, emit)
    }
}

/// How much older history one explicit request loads for a repository.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HistoryRequest {
    /// One more page of every active branch.
    Older,
    /// Keep paging until the history is complete, incomplete, or cancelled.
    All,
}

/// Request from the application to the backlog session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BacklogEffect {
    Load {
        generation: u64,
        repository_id: u64,
        request: HistoryRequest,
    },
    Cancel {
        generation: u64,
    },
}

/// How a history operation ended. Loaded commits are always retained.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HistoryOutcome {
    Finished,
    Cancelled,
    DiscoveryFailed(LoadFailure),
    UnknownRepository,
}

/// Bounded progress and results of backlog loading. Every event carries the
/// generation of the operation that produced it so stale events from a
/// cancelled or replaced operation can be ignored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BacklogEvent {
    DiscoveryPage {
        generation: u64,
        page: usize,
        owned_repositories: usize,
    },
    Discovered {
        generation: u64,
        repositories: Vec<RepositoryIdentity>,
    },
    Progress {
        generation: u64,
        repository_id: u64,
        pages_fetched: usize,
        commits_loaded: usize,
    },
    Snapshot {
        generation: u64,
        repository: Repository,
        coverage: HistoryCoverage,
        failures: Vec<LoadFailure>,
        pages_loaded: usize,
    },
    Finished {
        generation: u64,
        repository_id: Option<u64>,
        outcome: HistoryOutcome,
    },
}

impl BacklogEvent {
    pub fn generation(&self) -> u64 {
        match self {
            Self::DiscoveryPage { generation, .. }
            | Self::Discovered { generation, .. }
            | Self::Progress { generation, .. }
            | Self::Snapshot { generation, .. }
            | Self::Finished { generation, .. } => *generation,
        }
    }
}

struct LoadCommand {
    generation: u64,
    repository_id: u64,
    request: HistoryRequest,
    cancellation: CancellationToken,
}

/// Runs a history pager on a background worker. The worker loads the initial
/// batch (discovery plus the first page of every branch), then waits for
/// explicit Load older / Load all commands. The UI never blocks on it.
pub struct BacklogSession {
    receiver: Option<Receiver<BacklogEvent>>,
    commands: Option<SyncSender<LoadCommand>>,
    shutdown: CancellationToken,
    operations: HashMap<u64, CancellationToken>,
    worker: Option<JoinHandle<()>>,
}

impl BacklogSession {
    pub fn start<P: HistoryPager + Send + 'static>(pager: P) -> Self {
        let (sender, receiver) = sync_channel(EVENT_CHANNEL_CAPACITY);
        let (command_sender, command_receiver) = sync_channel(COMMAND_CHANNEL_CAPACITY);
        let shutdown = CancellationToken::default();
        let initial = CancellationToken::default();
        let worker_shutdown = shutdown.clone();
        let worker_initial = initial.clone();
        let worker = thread::spawn(move || {
            let mut worker = BacklogWorker {
                pager,
                sender,
                shutdown: worker_shutdown,
                login: String::new(),
                histories: Vec::new(),
            };
            if worker.initial(&worker_initial) {
                worker.serve(&command_receiver);
            }
        });
        Self {
            receiver: Some(receiver),
            commands: Some(command_sender),
            shutdown,
            operations: HashMap::from([(INITIAL_GENERATION, initial)]),
            worker: Some(worker),
        }
    }

    pub fn shutdown(&mut self) {
        self.shutdown.cancel();
        for (_, token) in self.operations.drain() {
            token.cancel();
        }
        self.commands.take();
        self.receiver.take();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

impl LoaderEventSource for BacklogSession {
    fn try_next(&mut self) -> Option<LoadEvent> {
        None
    }

    fn try_next_backlog(&mut self) -> Option<BacklogEvent> {
        let event = self.receiver.as_ref()?.try_recv().ok()?;
        if let BacklogEvent::Finished { generation, .. } = &event {
            self.operations.remove(generation);
        }
        Some(event)
    }

    fn request_history(&mut self, effect: BacklogEffect) -> bool {
        match effect {
            BacklogEffect::Load {
                generation,
                repository_id,
                request,
            } => {
                let Some(commands) = self.commands.as_ref() else {
                    return false;
                };
                let cancellation = CancellationToken::default();
                let command = LoadCommand {
                    generation,
                    repository_id,
                    request,
                    cancellation: cancellation.clone(),
                };
                if commands.try_send(command).is_err() {
                    return false;
                }
                self.operations.insert(generation, cancellation);
                true
            }
            BacklogEffect::Cancel { generation } => {
                if let Some(token) = self.operations.get(&generation) {
                    token.cancel();
                }
                true
            }
        }
    }

    fn cancel(&mut self) {
        self.shutdown.cancel();
        for token in self.operations.values() {
            token.cancel();
        }
    }
}

impl Drop for BacklogSession {
    fn drop(&mut self) {
        self.shutdown();
    }
}

struct BacklogWorker<P> {
    pager: P,
    sender: SyncSender<BacklogEvent>,
    shutdown: CancellationToken,
    login: String,
    histories: Vec<RepositoryHistory>,
}

impl<P: HistoryPager> BacklogWorker<P> {
    fn send(&self, event: BacklogEvent) -> bool {
        send_backlog_event(&self.sender, &self.shutdown, event)
    }

    /// Discover repositories and fetch the first page of every branch. Returns
    /// whether the worker should continue serving commands.
    fn initial(&mut self, cancellation: &CancellationToken) -> bool {
        let generation = INITIAL_GENERATION;
        let sender = self.sender.clone();
        let shutdown = self.shutdown.clone();
        let discovery = self.pager.discover(cancellation, &mut |event| {
            if let HistoryEvent::DiscoveryPage {
                page,
                owned_repositories,
            } = event
            {
                send_backlog_event(
                    &sender,
                    &shutdown,
                    BacklogEvent::DiscoveryPage {
                        generation,
                        page,
                        owned_repositories,
                    },
                );
            }
        });
        let discovery = match discovery {
            Ok(discovery) => discovery,
            Err(failure) => {
                let outcome = if failure.category == FailureCategory::Cancelled {
                    HistoryOutcome::Cancelled
                } else {
                    HistoryOutcome::DiscoveryFailed(failure)
                };
                self.send(BacklogEvent::Finished {
                    generation,
                    repository_id: None,
                    outcome,
                });
                return false;
            }
        };
        self.login = discovery.login;
        self.histories = discovery.repositories;
        if !self.send(BacklogEvent::Discovered {
            generation,
            repositories: self
                .histories
                .iter()
                .map(|history| history.identity.clone())
                .collect(),
        }) {
            return false;
        }

        let mut pages_fetched = 0;
        let mut outcome = HistoryOutcome::Finished;
        for index in 0..self.histories.len() {
            if cancellation.is_cancelled() {
                outcome = HistoryOutcome::Cancelled;
                break;
            }
            if self
                .fetch_pass(index, generation, cancellation, &mut pages_fetched)
                .is_err()
            {
                // Publish any page of this repository merged before cancelling.
                self.send_snapshot(index, generation);
                outcome = HistoryOutcome::Cancelled;
                break;
            }
        }
        self.send(BacklogEvent::Finished {
            generation,
            repository_id: None,
            outcome,
        })
    }

    fn serve(&mut self, commands: &Receiver<LoadCommand>) {
        while let Ok(command) = commands.recv() {
            if self.shutdown.is_cancelled() {
                return;
            }
            if !self.handle(command) {
                return;
            }
        }
    }

    fn handle(&mut self, command: LoadCommand) -> bool {
        let LoadCommand {
            generation,
            repository_id,
            request,
            cancellation,
        } = command;
        let Some(index) = self
            .histories
            .iter()
            .position(|history| history.identity.id == repository_id)
        else {
            return self.send(BacklogEvent::Finished {
                generation,
                repository_id: Some(repository_id),
                outcome: HistoryOutcome::UnknownRepository,
            });
        };
        // An explicit request retries branches whose previous page failed;
        // merged commits and successful cursors are kept.
        self.histories[index].retry_failed();
        let mut pages_fetched = 0;
        let mut outcome = HistoryOutcome::Finished;
        loop {
            if cancellation.is_cancelled() {
                outcome = HistoryOutcome::Cancelled;
                break;
            }
            if self
                .fetch_pass(index, generation, &cancellation, &mut pages_fetched)
                .is_err()
            {
                outcome = HistoryOutcome::Cancelled;
                break;
            }
            if request == HistoryRequest::Older || !self.histories[index].has_pending_pages() {
                break;
            }
        }
        if outcome == HistoryOutcome::Cancelled {
            // Publish every page merged before cancellation.
            self.send_snapshot(index, generation);
        }
        self.send(BacklogEvent::Finished {
            generation,
            repository_id: Some(repository_id),
            outcome,
        })
    }

    /// One page round for a repository followed by a snapshot. `Err` means
    /// the operation was cancelled; merged pages are kept.
    fn fetch_pass(
        &mut self,
        index: usize,
        generation: u64,
        cancellation: &CancellationToken,
        pages_fetched: &mut usize,
    ) -> Result<(), ()> {
        let repository_id = self.histories[index].identity.id;
        let sender = self.sender.clone();
        let shutdown = self.shutdown.clone();
        let base_pages = *pages_fetched;
        let mut fetched_here = 0;
        let result = self.pager.fetch_next_pages(
            &mut self.histories[index],
            &self.login,
            cancellation,
            &mut |event| {
                if let HistoryEvent::HistoryPage { total_commits, .. } = event {
                    fetched_here += 1;
                    send_backlog_event(
                        &sender,
                        &shutdown,
                        BacklogEvent::Progress {
                            generation,
                            repository_id,
                            pages_fetched: base_pages + fetched_here,
                            commits_loaded: total_commits,
                        },
                    );
                }
            },
        );
        *pages_fetched = base_pages + fetched_here;
        match result {
            Ok(_) => {
                self.send_snapshot(index, generation);
                Ok(())
            }
            Err(_) => Err(()),
        }
    }

    fn send_snapshot(&self, index: usize, generation: u64) {
        let history = &self.histories[index];
        self.send(BacklogEvent::Snapshot {
            generation,
            repository: history.repository(),
            coverage: history.coverage(),
            failures: history.failures(),
            pages_loaded: history
                .branches
                .iter()
                .map(|branch| branch.next_page.saturating_sub(1))
                .sum(),
        });
    }
}

/// Deliver an event through the bounded channel, waiting while it is full.
/// Operation cancellation does not drop events, so the final snapshot of a
/// cancelled load still arrives; only session shutdown or a disconnected
/// receiver stops delivery. Returns whether the receiver is still attached.
fn send_backlog_event(
    sender: &SyncSender<BacklogEvent>,
    shutdown: &CancellationToken,
    mut event: BacklogEvent,
) -> bool {
    loop {
        if shutdown.is_cancelled() {
            return false;
        }
        match sender.try_send(event) {
            Ok(()) => return true,
            Err(TrySendError::Disconnected(_)) => return false,
            Err(TrySendError::Full(returned)) => {
                event = returned;
                thread::sleep(Duration::from_millis(5));
            }
        }
    }
}

/// Sanitized cancellation failure used by in-process pagers.
pub fn cancelled_failure(repository_index: usize) -> LoadFailure {
    LoadFailure {
        category: FailureCategory::Cancelled,
        scope: FailureScope::Repository { repository_index },
        http_status: None,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::ffi::{OsStr, OsString};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::Instant;

    use chrono::{TimeZone, Utc};
    use serde_json::{Value, json};

    use super::*;
    use crate::app::{App, Input};
    use crate::comment_draft::MemoryDraftStore;
    use crate::github::{
        BoundedBytes, BranchCursor, BranchEnumeration, ProcessError, ProcessOutput,
    };
    use crate::inbox::{BacklogOrigin, ChildPane, Commit, GitHubAuthor, Inbox};
    use crate::review_state::MemoryReviewStore;

    struct Step {
        endpoint: String,
        fields: Vec<String>,
        status: u16,
        headers: Vec<(&'static str, &'static str)>,
        body: String,
    }

    /// Fictional `gh api` responses in request order. Unexpected requests
    /// panic the worker, which the tests observe as a missing result.
    #[derive(Clone, Default)]
    struct ScriptedGh {
        steps: Arc<Mutex<VecDeque<Step>>>,
        calls: Arc<Mutex<usize>>,
    }

    impl ScriptedGh {
        fn push(&self, endpoint: &str, fields: &[String], body: Value) {
            self.push_status(endpoint, fields, 200, Vec::new(), body);
        }

        fn push_status(
            &self,
            endpoint: &str,
            fields: &[String],
            status: u16,
            headers: Vec<(&'static str, &'static str)>,
            body: Value,
        ) {
            self.steps.lock().unwrap().push_back(Step {
                endpoint: endpoint.to_owned(),
                fields: fields.to_vec(),
                status,
                headers,
                body: body.to_string(),
            });
        }

        fn remaining(&self) -> usize {
            self.steps.lock().unwrap().len()
        }

        fn calls(&self) -> usize {
            *self.calls.lock().unwrap()
        }
    }

    impl ProcessRunner for ScriptedGh {
        fn run(
            &self,
            executable: &OsStr,
            arguments: &[OsString],
        ) -> Result<ProcessOutput, ProcessError> {
            let arguments = arguments
                .iter()
                .map(|argument| argument.to_string_lossy().into_owned())
                .collect::<Vec<_>>();
            assert_eq!(executable, OsStr::new("gh"));
            assert_eq!(&arguments[..4], ["api", "--method", "GET", "--include"]);
            assert!(
                !arguments.iter().any(
                    |argument| argument.starts_with("since=") || argument.starts_with("until=")
                )
            );
            let step = self
                .steps
                .lock()
                .unwrap()
                .pop_front()
                .expect("unexpected gh request");
            assert_eq!(arguments[4], step.endpoint);
            let fields = arguments[5..]
                .chunks(2)
                .map(|pair| pair[1].clone())
                .collect::<Vec<_>>();
            assert_eq!(fields, step.fields);
            *self.calls.lock().unwrap() += 1;
            let mut stdout = format!("HTTP/2.0 {} status\r\n", step.status).into_bytes();
            for (name, value) in step.headers {
                stdout.extend_from_slice(format!("{name}: {value}\r\n").as_bytes());
            }
            stdout.extend_from_slice(b"\r\n");
            stdout.extend_from_slice(step.body.as_bytes());
            let success = step.status == 200;
            Ok(ProcessOutput {
                success,
                exit_code: Some(if success { 0 } else { 1 }),
                stdout: BoundedBytes {
                    bytes: stdout,
                    truncated: false,
                },
                stderr: BoundedBytes {
                    bytes: Vec::new(),
                    truncated: false,
                },
            })
        }
    }

    const COMMITS: &str = "/repos/Octo/archive/commits";

    fn sha(value: usize) -> String {
        format!("{value:040x}")
    }

    fn history_fields(branch: &str, page: usize) -> Vec<String> {
        vec![
            format!("sha={branch}"),
            "author=Octo".to_owned(),
            "per_page=100".to_owned(),
            format!("page={page}"),
        ]
    }

    fn api_commit(value: usize, login: &str, date: &str) -> Value {
        json!({
            "sha": sha(value),
            "author": {"login": login},
            "commit": {"author": {"date": date}, "message": format!("Fictional change {value}")}
        })
    }

    fn page(range: std::ops::Range<usize>, login: &str) -> Value {
        Value::Array(
            range
                .map(|value| api_commit(value, login, "2024-01-10T10:00:00Z"))
                .collect(),
        )
    }

    fn discovery(gh: &ScriptedGh, branches: &[&str]) {
        gh.push("/user", &[], json!({"login": "Octo"}));
        gh.push(
            "/user/repos",
            &[
                "affiliation=owner".to_owned(),
                "per_page=100".to_owned(),
                "page=1".to_owned(),
            ],
            json!([{"id": 7, "name": "archive", "owner": {"login": "Octo"}}]),
        );
        gh.push(
            "/repos/Octo/archive/branches",
            &["per_page=100".to_owned(), "page=1".to_owned()],
            Value::Array(branches.iter().map(|name| json!({"name": name})).collect()),
        );
    }

    fn app() -> App {
        App::with_stores(
            Inbox::backlog(BacklogOrigin::GitHub),
            Box::new(MemoryReviewStore::default()),
            Box::new(MemoryDraftStore::default()),
        )
    }

    fn pump_until(app: &mut App, session: &mut BacklogSession, done: impl Fn(&App) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            for effect in app.take_backlog_effects() {
                if !session.request_history(effect.clone()) {
                    app.reject_backlog_effect(effect);
                }
            }
            while let Some(event) = session.try_next_backlog() {
                app.apply_backlog_event(event);
            }
            if done(app) {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "backlog worker did not reach the expected state"
            );
            thread::sleep(Duration::from_millis(1));
        }
    }

    fn idle(app: &App) -> bool {
        app.backlog_state()
            .is_some_and(|state| state.active().is_none())
    }

    fn coverage(app: &App) -> HistoryCoverage {
        app.backlog_state().unwrap().coverage(7)
    }

    fn pending_shas(app: &App) -> Vec<String> {
        app.current_commits()
            .iter()
            .map(|commit| commit.sha.clone())
            .collect()
    }

    #[test]
    fn empty_accepted_pages_do_not_hide_an_old_pending_commit() {
        let gh = ScriptedGh::default();
        discovery(&gh, &["main"]);
        // Full raw pages whose accepted projection is empty.
        gh.push(COMMITS, &history_fields("main", 1), page(0..100, "Other"));
        gh.push(COMMITS, &history_fields("main", 2), page(100..200, "Other"));
        gh.push(
            COMMITS,
            &history_fields("main", 3),
            json!([api_commit(500, "Octo", "2016-02-29T08:00:00Z")]),
        );

        let mut app = app();
        let mut session = BacklogSession::start(GitHubLoader::new(gh.clone()));
        pump_until(&mut app, &mut session, idle);
        assert_eq!(app.visible_repositories().len(), 1);
        assert!(pending_shas(&app).is_empty());
        assert_eq!(coverage(&app), HistoryCoverage::MoreAvailable);
        assert!(!app.backlog_clear());

        app.handle_input(Input::Character('o'));
        pump_until(&mut app, &mut session, idle);
        assert!(pending_shas(&app).is_empty());
        assert_eq!(coverage(&app), HistoryCoverage::MoreAvailable);

        app.handle_input(Input::Character('O'));
        pump_until(&mut app, &mut session, idle);
        assert_eq!(pending_shas(&app), [sha(500)]);
        assert_eq!(coverage(&app), HistoryCoverage::Complete);
        assert!(app.status().contains("history complete"));

        // A genuinely exhausted branch is never paged again.
        app.handle_input(Input::Character('o'));
        pump_until(&mut app, &mut session, idle);
        assert_eq!(gh.remaining(), 0);
        assert_eq!(gh.calls(), 6);
        session.shutdown();
    }

    #[test]
    fn load_all_pages_overlapping_branches_and_deduplicates_shas() {
        let gh = ScriptedGh::default();
        discovery(&gh, &["main", "feature"]);
        gh.push(
            COMMITS,
            &history_fields("feature", 1),
            page(50..150, "Octo"),
        );
        gh.push(COMMITS, &history_fields("main", 1), page(0..100, "octo"));
        gh.push(
            COMMITS,
            &history_fields("feature", 2),
            json!([
                api_commit(200, "Octo", "2019-01-01T00:00:00Z"),
                api_commit(201, "Octo", "2018-01-01T00:00:00Z")
            ]),
        );
        gh.push(
            COMMITS,
            &history_fields("main", 2),
            json!([api_commit(200, "Octo", "2019-01-01T00:00:00Z")]),
        );

        let mut app = app();
        let mut session = BacklogSession::start(GitHubLoader::new(gh.clone()));
        pump_until(&mut app, &mut session, idle);
        assert_eq!(app.loaded_commit_count(), 150);
        assert_eq!(coverage(&app), HistoryCoverage::MoreAvailable);

        app.handle_input(Input::Enter);
        let selected = app.current_commit().unwrap().sha.clone();
        app.handle_input(Input::Character('O'));
        pump_until(&mut app, &mut session, idle);
        assert_eq!(app.loaded_commit_count(), 152);
        assert_eq!(coverage(&app), HistoryCoverage::Complete);
        assert_eq!(app.current_commit().unwrap().sha, selected);
        assert!(app.status().contains("2 new commits"));
        assert_eq!(gh.remaining(), 0);
        session.shutdown();
    }

    #[test]
    fn rate_limited_branch_leaves_history_incomplete_until_retried() {
        let gh = ScriptedGh::default();
        discovery(&gh, &["main", "feature"]);
        gh.push(COMMITS, &history_fields("feature", 1), page(0..100, "Octo"));
        gh.push(COMMITS, &history_fields("main", 1), page(100..200, "Octo"));
        gh.push_status(
            COMMITS,
            &history_fields("feature", 2),
            403,
            vec![("x-ratelimit-remaining", "0")],
            json!({"message": "private fictional detail"}),
        );
        gh.push(
            COMMITS,
            &history_fields("main", 2),
            json!([api_commit(300, "Octo", "2017-05-05T00:00:00Z")]),
        );

        let mut app = app();
        let mut session = BacklogSession::start(GitHubLoader::new(gh.clone()));
        pump_until(&mut app, &mut session, idle);
        app.handle_input(Input::Character('O'));
        pump_until(&mut app, &mut session, idle);
        assert_eq!(coverage(&app), HistoryCoverage::Incomplete);
        assert_eq!(app.loaded_commit_count(), 201);
        assert!(
            app.status()
                .contains("history incomplete: GitHub API rate limit reached")
        );
        assert!(!app.status().contains("private"));
        assert!(!app.backlog_clear());

        gh.push(
            COMMITS,
            &history_fields("feature", 2),
            json!([api_commit(301, "Octo", "2015-05-05T00:00:00Z")]),
        );
        app.handle_input(Input::Character('o'));
        pump_until(&mut app, &mut session, idle);
        assert_eq!(coverage(&app), HistoryCoverage::Complete);
        assert_eq!(app.loaded_commit_count(), 202);
        assert_eq!(gh.remaining(), 0);
        session.shutdown();
    }

    #[test]
    fn discovery_failure_finishes_the_initial_load_without_repositories() {
        let gh = ScriptedGh::default();
        gh.push_status("/user", &[], 401, Vec::new(), json!({"message": "no"}));
        let mut app = app();
        let mut session = BacklogSession::start(GitHubLoader::new(gh.clone()));
        pump_until(&mut app, &mut session, idle);
        assert!(app.status().starts_with("LOAD FAILED"));
        assert!(!app.backlog_clear());
        app.handle_input(Input::Character('o'));
        assert!(app.take_backlog_effects().is_empty());
        session.shutdown();
    }

    /// Fictional single-branch pager; page 3 blocks until cancelled the first
    /// time it is requested.
    struct GatedPager {
        gate_closed: Arc<AtomicBool>,
        pages: usize,
    }

    fn fictional_commit(value: usize) -> Commit {
        Commit {
            sha: sha(value),
            subject: format!("Fictional page commit {value}"),
            author: GitHubAuthor {
                login: "octo".to_owned(),
            },
            authored_at: Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap()
                - chrono::TimeDelta::days(value as i64 * 90),
            files: ChildPane::Unavailable,
        }
    }

    impl HistoryPager for GatedPager {
        fn discover(
            &mut self,
            _cancellation: &CancellationToken,
            _emit: &mut dyn FnMut(HistoryEvent),
        ) -> Result<HistoryDiscovery, LoadFailure> {
            Ok(HistoryDiscovery {
                login: "octo".to_owned(),
                repositories: vec![RepositoryHistory::new(
                    RepositoryIdentity {
                        id: 7,
                        owner: "octo".to_owned(),
                        name: "gated".to_owned(),
                    },
                    0,
                )],
            })
        }

        fn fetch_next_pages(
            &mut self,
            history: &mut RepositoryHistory,
            _login: &str,
            cancellation: &CancellationToken,
            emit: &mut dyn FnMut(HistoryEvent),
        ) -> Result<HistoryPass, LoadFailure> {
            if history.enumeration == BranchEnumeration::Pending {
                history.enumeration = BranchEnumeration::Loaded;
                history.branches = vec![BranchCursor {
                    name: "main".to_owned(),
                    next_page: 1,
                    exhausted: false,
                    failed: None,
                }];
            }
            let page = history.branches[0].next_page;
            if page == 3 && self.gate_closed.swap(false, Ordering::SeqCst) {
                while !cancellation.is_cancelled() {
                    thread::sleep(Duration::from_millis(1));
                }
                return Err(cancelled_failure(0));
            }
            history.commits.insert(sha(page), fictional_commit(page));
            history.branches[0].next_page += 1;
            history.branches[0].exhausted = page == self.pages;
            emit(HistoryEvent::HistoryPage {
                repository_index: 0,
                branch_index: 0,
                page,
                accepted_commits: 1,
                total_commits: history.commits.len(),
            });
            Ok(HistoryPass {
                pages_fetched: 1,
                accepted_commits: 1,
                coverage: history.coverage(),
            })
        }
    }

    #[test]
    fn cancelling_load_all_keeps_loaded_pages_and_allows_a_later_load() {
        let gate_closed = Arc::new(AtomicBool::new(true));
        let mut app = app();
        let mut session = BacklogSession::start(GatedPager {
            gate_closed: Arc::clone(&gate_closed),
            pages: 4,
        });
        pump_until(&mut app, &mut session, idle);
        assert_eq!(app.loaded_commit_count(), 1);

        app.handle_input(Input::Character('O'));
        pump_until(&mut app, &mut session, |app| {
            app.backlog_state()
                .and_then(|state| state.active())
                .is_some_and(|active| active.pages_fetched == 1)
        });
        // Page 3 is now blocked in flight; the UI stays responsive.
        assert!(app.status().contains("x cancels"));
        app.handle_input(Input::Character('x'));
        pump_until(&mut app, &mut session, idle);
        assert_eq!(
            app.status(),
            "Cancelled — 2 commits loaded, history incomplete"
        );
        assert_eq!(app.loaded_commit_count(), 2);
        assert_eq!(coverage(&app), HistoryCoverage::MoreAvailable);
        assert!(!gate_closed.load(Ordering::SeqCst));

        app.handle_input(Input::Character('O'));
        pump_until(&mut app, &mut session, idle);
        assert_eq!(app.loaded_commit_count(), 4);
        assert_eq!(coverage(&app), HistoryCoverage::Complete);
        session.shutdown();
    }

    #[test]
    fn shutdown_interrupts_a_blocked_load_and_a_full_event_channel() {
        let mut app = app();
        let mut session = BacklogSession::start(GatedPager {
            gate_closed: Arc::new(AtomicBool::new(true)),
            pages: 10,
        });
        pump_until(&mut app, &mut session, idle);
        app.handle_input(Input::Character('O'));
        pump_until(&mut app, &mut session, |app| {
            app.backlog_state()
                .and_then(|state| state.active())
                .is_some_and(|active| active.pages_fetched == 1)
        });
        let started = Instant::now();
        session.shutdown();
        assert!(started.elapsed() < Duration::from_secs(5));

        // Never drain events: the worker fills the bounded channel and must
        // still stop on shutdown.
        let mut session = BacklogSession::start(GatedPager {
            gate_closed: Arc::new(AtomicBool::new(false)),
            pages: 1_000,
        });
        assert!(session.request_history(BacklogEffect::Load {
            generation: 2,
            repository_id: 7,
            request: HistoryRequest::All,
        }));
        thread::sleep(Duration::from_millis(50));
        let started = Instant::now();
        drop(session);
        assert!(started.elapsed() < Duration::from_secs(5));
    }
}
