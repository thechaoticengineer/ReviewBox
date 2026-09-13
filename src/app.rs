use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::Arc;

use ratatui::layout::Rect;

use crate::comment_draft::{CommentDrafts, DraftStateError, DraftStore, MemoryDraftStore};
use crate::github::{
    DetailFailure, DetailState, FailureCategory, LoadEvent, LoadFailure, LoadProgress, LoadStatus,
    LoadedRepository, RepositoryCoverage,
};
use crate::inbox::{
    Commit, CommitDetail, DiffLine, FileChange, Inbox, InboxSource, PatchCapReason, PatchContent,
    Repository, RepositoryIdentity,
};
use crate::review_state::{
    MemoryReviewStore, ReviewKey, ReviewMarks, ReviewStateError, ReviewStore,
};
use crate::ui_layout::{ReviewPaneLayout, wrap_text};

pub const MIN_FULL_WIDTH: u16 = 60;
pub const MIN_FULL_HEIGHT: u16 = 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Normal,
    SearchEntry { target: Pane },
    Help { previous_focus: Pane },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Pane {
    Repository,
    Commit,
    File,
    Diff,
}

impl Pane {
    pub const fn title(self) -> &'static str {
        match self {
            Self::Repository => "Repository",
            Self::Commit => "Commit",
            Self::File => "File",
            Self::Diff => "Diff",
        }
    }

    const fn index(self) -> usize {
        match self {
            Self::Repository => 0,
            Self::Commit => 1,
            Self::File => 2,
            Self::Diff => 3,
        }
    }

    const fn previous(self) -> Option<Self> {
        match self {
            Self::Repository => None,
            Self::Commit => Some(Self::Repository),
            Self::File => Some(Self::Commit),
            Self::Diff => Some(Self::File),
        }
    }

    const fn next(self) -> Option<Self> {
        match self {
            Self::Repository => Some(Self::Commit),
            Self::Commit => Some(Self::File),
            Self::File => Some(Self::Diff),
            Self::Diff => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    Quit,
    FocusPrevious,
    FocusNext,
    MoveDown,
    MoveUp,
    GPrefix,
    Last,
    HalfPageDown,
    HalfPageUp,
    Open,
    Back,
    ToggleReviewed,
    ToggleRemaining,
    SearchNext,
    SearchPrevious,
    Unrelated,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Input {
    Character(char),
    Backspace,
    Enter,
    Escape,
    HalfPageDown,
    HalfPageUp,
    Quit,
    Unrelated,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HelpBinding {
    pub keys: &'static str,
    pub action: &'static str,
}

pub const HELP_BINDINGS: &[HelpBinding] = &[
    HelpBinding {
        keys: "h / l",
        action: "focus previous / next pane",
    },
    HelpBinding {
        keys: "j / k",
        action: "move or scroll down / up",
    },
    HelpBinding {
        keys: "gg / G",
        action: "first / last position",
    },
    HelpBinding {
        keys: "Ctrl-d / Ctrl-u",
        action: "move down / up half a pane",
    },
    HelpBinding {
        keys: "Enter / Escape",
        action: "open child / return to parent",
    },
    HelpBinding {
        keys: "/",
        action: "search the focused pane",
    },
    HelpBinding {
        keys: "n / N",
        action: "next / previous search match",
    },
    HelpBinding {
        keys: "m",
        action: "mark commit reviewed / unreviewed",
    },
    HelpBinding {
        keys: "f",
        action: "show remaining / all commits",
    },
    HelpBinding {
        keys: "?",
        action: "open this help",
    },
    HelpBinding {
        keys: "q / Ctrl-c",
        action: "quit from normal mode / quit globally",
    },
    HelpBinding {
        keys: "Search: text, Backspace",
        action: "edit the query",
    },
    HelpBinding {
        keys: "Search: Enter / Escape",
        action: "apply / cancel",
    },
    HelpBinding {
        keys: "Help: Escape",
        action: "close help",
    },
];

#[derive(Debug, Default, Clone, Copy)]
struct ListPosition {
    selected: usize,
    scroll: usize,
}

#[derive(Debug, Clone)]
struct SearchQuery {
    original: String,
    needle: String,
}

const MAX_VISIBLE_FAILURES: usize = 3;
const DETAIL_CACHE_CAPACITY: usize = 16;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DetailKey {
    pub repository_id: u64,
    pub sha: String,
}

#[derive(Debug, Clone)]
pub enum DetailEffect {
    Request {
        request_id: u64,
        key: DetailKey,
        repository: RepositoryIdentity,
    },
    Cancel {
        request_id: u64,
    },
}

#[derive(Debug)]
pub struct DetailResult {
    pub request_id: u64,
    pub key: DetailKey,
    pub outcome: Result<CommitDetail, DetailFailure>,
}

#[derive(Debug, Clone)]
struct ActiveDetailRequest {
    request_id: u64,
    key: DetailKey,
}

#[derive(Debug, Clone)]
struct VisibleRepository {
    inbox_index: usize,
    commit_indices: Vec<usize>,
}

#[derive(Debug, Clone)]
struct SelectionIdentity {
    repository_id: Option<u64>,
    commit_sha: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LivePhase {
    Authenticating,
    Discovering {
        page: usize,
        owned_repositories: usize,
    },
    LoadingRepository {
        repository: usize,
        total: usize,
    },
    LoadingBranches {
        repository: usize,
        page: usize,
        branches: usize,
    },
    LoadingCommits {
        repository: usize,
        branch: usize,
        branches: usize,
        page: usize,
        accepted_commits: usize,
    },
    Complete,
    EmptyDay,
    NoOwnedRepositories,
    Incomplete,
    Fatal,
}

#[derive(Debug)]
pub struct LiveInboxState {
    pub phase: LivePhase,
    pub progress: LoadProgress,
    failures: Vec<LoadFailure>,
    omitted_failures: usize,
    loaded: BTreeMap<usize, LoadedRepository>,
    branch_totals: BTreeMap<usize, usize>,
    branches_done: BTreeMap<usize, usize>,
}

impl LiveInboxState {
    fn new() -> Self {
        Self {
            phase: LivePhase::Authenticating,
            progress: LoadProgress::default(),
            failures: Vec::new(),
            omitted_failures: 0,
            loaded: BTreeMap::new(),
            branch_totals: BTreeMap::new(),
            branches_done: BTreeMap::new(),
        }
    }

    pub fn failures(&self) -> &[LoadFailure] {
        &self.failures
    }

    pub fn omitted_failures(&self) -> usize {
        self.omitted_failures
    }

    pub fn available_commits(&self) -> usize {
        self.loaded
            .values()
            .map(|repository| repository.repository.commits.len())
            .sum()
    }

    pub fn incomplete_repositories(&self) -> usize {
        self.loaded
            .values()
            .filter(|repository| repository.coverage != RepositoryCoverage::Complete)
            .count()
    }
}

#[derive(Debug)]
pub struct App {
    inbox: Inbox,
    pub terminal_width: u16,
    pub terminal_height: u16,
    mode: Mode,
    focus: Pane,
    repositories: ListPosition,
    commits: ListPosition,
    files: ListPosition,
    diff_scroll: usize,
    diff_row_offset: usize,
    diff_match: Option<usize>,
    diff_viewport_width: usize,
    viewport_heights: [usize; 4],
    pending_g: bool,
    search_query: String,
    last_search: Option<SearchQuery>,
    status: String,
    should_quit: bool,
    live: Option<LiveInboxState>,
    review_store: Option<Box<dyn ReviewStore>>,
    review_marks: ReviewMarks,
    review_warning: Option<String>,
    draft_store: Option<Box<dyn DraftStore>>,
    drafts: CommentDrafts,
    draft_warning: Option<String>,
    remaining_only: bool,
    visible: Vec<VisibleRepository>,
    detail_cache: HashMap<DetailKey, DetailState>,
    detail_lru: VecDeque<DetailKey>,
    active_request: Option<ActiveDetailRequest>,
    next_request_id: u64,
    effects: VecDeque<DetailEffect>,
}

impl App {
    pub fn new(inbox: Inbox) -> Self {
        Self::with_stores(
            inbox,
            Box::new(MemoryReviewStore::default()),
            Box::new(MemoryDraftStore::default()),
        )
    }

    pub fn with_review_store(inbox: Inbox, store: Box<dyn ReviewStore>) -> Self {
        Self::with_stores(inbox, store, Box::new(MemoryDraftStore::default()))
    }

    pub fn with_stores(
        inbox: Inbox,
        review_store: Box<dyn ReviewStore>,
        draft_store: Box<dyn DraftStore>,
    ) -> Self {
        Self::with_store_results(inbox, Ok(review_store), Ok(draft_store))
    }

    #[cfg(test)]
    pub fn with_review_store_result(
        inbox: Inbox,
        store: Result<Box<dyn ReviewStore>, ReviewStateError>,
    ) -> Self {
        Self::with_store_results(inbox, store, Ok(Box::new(MemoryDraftStore::default())))
    }

    pub fn with_store_results(
        inbox: Inbox,
        review_store: Result<Box<dyn ReviewStore>, ReviewStateError>,
        draft_store: Result<Box<dyn DraftStore>, DraftStateError>,
    ) -> Self {
        let (review_store, review_marks, review_warning) = match review_store {
            Ok(store) => match store.load() {
                Ok(marks) => (Some(store), marks, None),
                Err(error) => (None, ReviewMarks::default(), Some(error.to_string())),
            },
            Err(error) => (None, ReviewMarks::default(), Some(error.to_string())),
        };
        let (draft_store, drafts, draft_warning) = match draft_store {
            Ok(store) => match store.load() {
                Ok(drafts) => (Some(store), drafts, None),
                Err(error) => (None, CommentDrafts::default(), Some(error.to_string())),
            },
            Err(error) => (None, CommentDrafts::default(), Some(error.to_string())),
        };
        Self::build(
            inbox,
            review_store,
            review_marks,
            review_warning,
            draft_store,
            drafts,
            draft_warning,
        )
    }

    fn build(
        inbox: Inbox,
        review_store: Option<Box<dyn ReviewStore>>,
        review_marks: ReviewMarks,
        review_warning: Option<String>,
        draft_store: Option<Box<dyn DraftStore>>,
        drafts: CommentDrafts,
        draft_warning: Option<String>,
    ) -> Self {
        let status = match &inbox.source {
            InboxSource::Demo => "Offline fictional demo".to_owned(),
            InboxSource::Live { .. } => "GitHub loading started".to_owned(),
        };
        let live = matches!(inbox.source, InboxSource::Live { .. }).then(LiveInboxState::new);
        let mut app = Self {
            inbox,
            terminal_width: 0,
            terminal_height: 0,
            mode: Mode::Normal,
            focus: Pane::Repository,
            repositories: ListPosition::default(),
            commits: ListPosition::default(),
            files: ListPosition::default(),
            diff_scroll: 0,
            diff_row_offset: 0,
            diff_match: None,
            diff_viewport_width: 0,
            viewport_heights: [0; 4],
            pending_g: false,
            search_query: String::new(),
            last_search: None,
            status,
            should_quit: false,
            live,
            review_store,
            review_marks,
            review_warning,
            draft_store,
            drafts,
            draft_warning,
            remaining_only: false,
            visible: Vec::new(),
            detail_cache: HashMap::new(),
            detail_lru: VecDeque::new(),
            active_request: None,
            next_request_id: 1,
            effects: VecDeque::new(),
        };
        app.rebuild_projection(None);
        app.normalize();
        app
    }

    pub fn resize(&mut self, width: u16, height: u16) {
        self.terminal_width = width;
        self.terminal_height = height;
        if width < MIN_FULL_WIDTH || height < MIN_FULL_HEIGHT {
            self.viewport_heights = [0; 4];
            self.diff_viewport_width = 0;
        } else {
            let layout = ReviewPaneLayout::from_area(Rect::new(0, 0, width, height));
            self.viewport_heights = layout.content_heights();
            self.diff_viewport_width = layout.diff_content_width();
        }
        self.normalize();
    }

    pub fn apply_load_event(&mut self, event: LoadEvent) {
        if self.live.is_none() {
            return;
        }

        let selection = self.selection_identity();

        let live = self.live.as_mut().expect("checked above");
        match event {
            LoadEvent::DiscoveryPage {
                page,
                owned_repositories,
            } => {
                live.progress.repositories_discovered = owned_repositories;
                live.phase = LivePhase::Discovering {
                    page,
                    owned_repositories,
                };
            }
            LoadEvent::RepositoriesDiscovered { total } => {
                live.progress.repositories_discovered = total;
                live.phase = LivePhase::LoadingRepository {
                    repository: 0,
                    total,
                };
            }
            LoadEvent::RepositoryStarted {
                repository_index,
                total,
            } => {
                live.phase = LivePhase::LoadingRepository {
                    repository: repository_index + 1,
                    total,
                };
            }
            LoadEvent::BranchPage {
                repository_index,
                page,
                branches,
            } => {
                live.branch_totals.insert(repository_index, branches);
                live.progress.branches_discovered = live.branch_totals.values().sum();
                live.phase = LivePhase::LoadingBranches {
                    repository: repository_index + 1,
                    page,
                    branches,
                };
            }
            LoadEvent::BranchStarted {
                repository_index,
                branch_index,
                total,
            } => {
                live.phase = LivePhase::LoadingCommits {
                    repository: repository_index + 1,
                    branch: branch_index + 1,
                    branches: total,
                    page: 0,
                    accepted_commits: 0,
                };
            }
            LoadEvent::CommitPage {
                repository_index,
                branch_index,
                page,
                accepted_commits,
            } => {
                let branches = match live.phase {
                    LivePhase::LoadingCommits { branches, .. } => branches,
                    _ => branch_index + 1,
                };
                live.phase = LivePhase::LoadingCommits {
                    repository: repository_index + 1,
                    branch: branch_index + 1,
                    branches,
                    page,
                    accepted_commits,
                };
            }
            LoadEvent::RepositorySnapshot {
                repository_index,
                repository,
            } => {
                if let LivePhase::LoadingCommits {
                    repository, branch, ..
                } = live.phase
                    && repository == repository_index + 1
                {
                    live.branches_done.insert(repository_index, branch);
                    live.progress.branches_processed = live.branches_done.values().sum();
                }
                live.loaded.insert(repository_index, repository);
                live.progress.commits_loaded = live.available_commits();
                self.inbox.repositories = live
                    .loaded
                    .values()
                    .map(|loaded| loaded.repository.clone())
                    .collect();
            }
            LoadEvent::RepositoryLoaded {
                repository_index,
                repository,
            } => {
                live.branch_totals
                    .insert(repository_index, repository.branch_count);
                live.branches_done
                    .insert(repository_index, repository.branch_count);
                live.loaded.insert(repository_index, repository);
                live.progress.repositories_processed = live
                    .progress
                    .repositories_processed
                    .max(repository_index + 1);
                live.progress.branches_discovered = live.branch_totals.values().sum();
                live.progress.branches_processed = live.branches_done.values().sum();
                live.progress.commits_loaded = live.available_commits();
                self.inbox.repositories = live
                    .loaded
                    .values()
                    .map(|loaded| loaded.repository.clone())
                    .collect();
            }
            LoadEvent::Failure(failure) => {
                if live.failures.len() < MAX_VISIBLE_FAILURES {
                    live.failures.push(failure);
                } else {
                    live.omitted_failures += 1;
                }
            }
            LoadEvent::Finished { status, progress } => {
                live.progress = progress;
                live.phase = match status {
                    LoadStatus::Fatal => LivePhase::Fatal,
                    LoadStatus::Incomplete => LivePhase::Incomplete,
                    LoadStatus::Complete if self.inbox.repositories.is_empty() => {
                        LivePhase::NoOwnedRepositories
                    }
                    LoadStatus::Complete
                        if self
                            .inbox
                            .repositories
                            .iter()
                            .all(|repository| repository.commits.is_empty()) =>
                    {
                        LivePhase::EmptyDay
                    }
                    LoadStatus::Complete => LivePhase::Complete,
                };
            }
        }

        self.rebuild_projection(Some(selection));
        self.clear_diff_match();
        self.cancel_detail_if_selection_changed();
        self.normalize();
    }

    pub fn apply(&mut self, command: Command) {
        if command == Command::GPrefix {
            if self.pending_g {
                self.pending_g = false;
                self.apply_normal(Command::GPrefix);
            } else {
                self.pending_g = true;
                self.status = "g: waiting for second g".to_owned();
            }
            self.normalize();
            return;
        }

        self.pending_g = false;
        match self.mode {
            Mode::Normal => self.apply_normal(command),
            Mode::SearchEntry { .. } | Mode::Help { .. } => {}
        }
        self.normalize();
    }

    pub fn handle_input(&mut self, input: Input) {
        if input == Input::Quit {
            self.pending_g = false;
            self.apply_normal(Command::Quit);
            return;
        }

        match self.mode {
            Mode::Normal => self.handle_normal_input(input),
            Mode::SearchEntry { target } => self.handle_search_input(target, input),
            Mode::Help { previous_focus } => self.handle_help_input(previous_focus, input),
        }
        self.normalize();
    }

    fn handle_normal_input(&mut self, input: Input) {
        let command = match input {
            Input::Character('/') => {
                self.pending_g = false;
                self.search_query.clear();
                self.mode = Mode::SearchEntry { target: self.focus };
                self.status = format!("Searching {}", self.focus.title());
                return;
            }
            Input::Character('?') => {
                self.pending_g = false;
                self.mode = Mode::Help {
                    previous_focus: self.focus,
                };
                self.status = "Keyboard help open".to_owned();
                return;
            }
            Input::Character('q') => Command::Quit,
            Input::Character('h') => Command::FocusPrevious,
            Input::Character('j') => Command::MoveDown,
            Input::Character('k') => Command::MoveUp,
            Input::Character('l') => Command::FocusNext,
            Input::Character('g') => Command::GPrefix,
            Input::Character('G') => Command::Last,
            Input::Character('m') => Command::ToggleReviewed,
            Input::Character('f') => Command::ToggleRemaining,
            Input::Character('n') => Command::SearchNext,
            Input::Character('N') => Command::SearchPrevious,
            Input::Enter => Command::Open,
            Input::Escape => Command::Back,
            Input::HalfPageDown => Command::HalfPageDown,
            Input::HalfPageUp => Command::HalfPageUp,
            Input::Character(_) | Input::Backspace | Input::Unrelated | Input::Quit => {
                Command::Unrelated
            }
        };
        self.apply(command);
    }

    fn handle_search_input(&mut self, target: Pane, input: Input) {
        self.pending_g = false;
        match input {
            Input::Character(character) => self.search_query.push(character),
            Input::Backspace => {
                self.search_query.pop();
            }
            Input::Enter => self.apply_search(target),
            Input::Escape => {
                self.search_query.clear();
                self.mode = Mode::Normal;
                self.status = "Search cancelled".to_owned();
            }
            Input::HalfPageDown | Input::HalfPageUp | Input::Unrelated | Input::Quit => {}
        }
    }

    fn handle_help_input(&mut self, previous_focus: Pane, input: Input) {
        self.pending_g = false;
        if input == Input::Escape {
            self.focus = previous_focus;
            self.mode = Mode::Normal;
            self.status = "Keyboard help closed".to_owned();
        }
    }

    fn apply_normal(&mut self, command: Command) {
        match command {
            Command::Quit => {
                self.cancel_active_detail();
                self.should_quit = true;
                self.status = "Closing ReviewBox".to_owned();
            }
            Command::FocusPrevious => self.focus_previous("Focus moved left"),
            Command::FocusNext => self.focus_next("Focus moved right"),
            Command::MoveDown => self.move_active(false, 1),
            Command::MoveUp => self.move_active(true, 1),
            Command::GPrefix => self.move_to_first(),
            Command::Last => self.move_to_last(),
            Command::HalfPageDown => self.move_active(false, self.half_page_step()),
            Command::HalfPageUp => self.move_active(true, self.half_page_step()),
            Command::Open => self.open_selected(),
            Command::Back => self.focus_previous("Returned to parent pane"),
            Command::ToggleReviewed => self.toggle_reviewed(),
            Command::ToggleRemaining => self.toggle_remaining(),
            Command::SearchNext => self.repeat_search(false),
            Command::SearchPrevious => self.repeat_search(true),
            Command::Unrelated => self.status = "Key has no action in normal mode".to_owned(),
        }
    }

    fn open_selected(&mut self) {
        if matches!(self.inbox.source, InboxSource::Live { .. }) && self.focus == Pane::Commit {
            self.open_live_commit();
        } else {
            self.focus_next("Opened selected item");
        }
    }

    fn open_live_commit(&mut self) {
        let Some((key, repository)) = self.current_detail_target() else {
            self.status = "No openable commit is selected".to_owned();
            return;
        };

        match self.detail_cache.get(&key) {
            Some(DetailState::Loading) => {
                self.focus = Pane::File;
                self.status = "Commit details are still loading".to_owned();
                return;
            }
            Some(DetailState::Ready(_)) => {
                self.touch_detail(&key);
                self.focus = Pane::File;
                self.status = "Opened cached commit details".to_owned();
                return;
            }
            Some(DetailState::NotRequested) | Some(DetailState::Failed(_)) | None => {}
        }

        self.cancel_active_detail();
        let request_id = self.next_request_id;
        self.next_request_id = self.next_request_id.saturating_add(1);
        self.detail_cache.insert(key.clone(), DetailState::Loading);
        self.touch_detail(&key);
        self.active_request = Some(ActiveDetailRequest {
            request_id,
            key: key.clone(),
        });
        self.effects.push_back(DetailEffect::Request {
            request_id,
            key,
            repository,
        });
        self.focus = Pane::File;
        self.status = "Loading commit details".to_owned();
    }

    fn toggle_reviewed(&mut self) {
        if self.focus == Pane::Repository {
            self.status = "Open a commit before changing review progress".to_owned();
            return;
        }
        let Some((repository_id, sha)) = self
            .current_repository()
            .zip(self.current_commit())
            .map(|(repository, commit)| (repository.identity.id, commit.sha.clone()))
        else {
            self.status = "No commit selected; review progress unchanged".to_owned();
            return;
        };
        let Ok(key) = ReviewKey::new(repository_id, &sha) else {
            self.status =
                "Selected commit identity is invalid; review progress unchanged".to_owned();
            return;
        };
        let Some(store) = self.review_store.as_ref() else {
            self.status = "Review progress is unavailable; mark unchanged".to_owned();
            return;
        };
        let reviewed = !self.review_marks.is_reviewed(&key);
        match store.set_reviewed(&key, reviewed) {
            Ok(marks) => {
                let selection = self.selection_identity();
                self.review_marks = marks;
                self.rebuild_projection(Some(selection));
                self.cancel_detail_if_selection_changed();
                self.status = if reviewed {
                    "Marked commit reviewed".to_owned()
                } else {
                    "Marked commit unreviewed".to_owned()
                };
            }
            Err(error) => {
                self.status = format!("Review mark unchanged: {error}");
            }
        }
    }

    fn toggle_remaining(&mut self) {
        let before = self.current_detail_key();
        let selection = self.selection_identity();
        self.remaining_only = !self.remaining_only;
        self.rebuild_projection(Some(selection));
        self.clear_diff_match();
        if self.focus > Pane::Commit && before != self.current_detail_key() {
            self.focus = Pane::Commit;
        }
        self.cancel_detail_if_selection_changed();
        self.status = if self.remaining_only {
            "Showing remaining commits only".to_owned()
        } else {
            "Showing all commits".to_owned()
        };
    }

    pub fn apply_detail_result(&mut self, result: DetailResult) {
        let is_current = self.active_request.as_ref().is_some_and(|active| {
            active.request_id == result.request_id && active.key == result.key
        });
        if !is_current {
            return;
        }
        self.active_request = None;

        match result.outcome {
            Ok(detail) => {
                let file_count = detail.files.len();
                self.detail_cache
                    .insert(result.key.clone(), DetailState::Ready(Arc::new(detail)));
                self.touch_detail(&result.key);
                self.status = if file_count == 0 {
                    "Commit details loaded; no changed files were returned".to_owned()
                } else {
                    format!("Commit details loaded ({file_count} files)")
                };
            }
            Err(DetailFailure::Load(failure)) if failure.category == FailureCategory::Cancelled => {
                self.detail_cache.remove(&result.key);
                self.remove_detail_lru(&result.key);
                self.status = "Commit detail request cancelled".to_owned();
            }
            Err(failure) => {
                self.detail_cache
                    .insert(result.key.clone(), DetailState::Failed(failure));
                self.touch_detail(&result.key);
                self.status = "Commit detail request failed".to_owned();
            }
        }
        self.clear_diff_match();
        self.normalize();
    }

    pub fn take_detail_effects(&mut self) -> Vec<DetailEffect> {
        self.effects.drain(..).collect()
    }

    fn apply_search(&mut self, target: Pane) {
        let query = std::mem::take(&mut self.search_query);
        self.mode = Mode::Normal;
        if query.is_empty() {
            self.status = "Empty search; position unchanged".to_owned();
            return;
        }

        self.last_search = Some(SearchQuery {
            needle: query.to_lowercase(),
            original: query,
        });
        self.search_current(target, false, true);
    }

    fn repeat_search(&mut self, backward: bool) {
        if self.last_search.is_none() {
            self.status = "No previous search".to_owned();
            return;
        }
        self.search_current(self.focus, backward, false);
    }

    fn search_current(&mut self, target: Pane, backward: bool, initial: bool) {
        let Some(query) = self.last_search.clone() else {
            return;
        };
        let current = match target {
            Pane::Repository => self.repositories.selected,
            Pane::Commit => self.commits.selected,
            Pane::File => self.files.selected,
            Pane::Diff => self.diff_match.unwrap_or(self.diff_scroll),
        };
        let found = self.find_match(target, current, backward);
        let direction = if backward { "previous" } else { "next" };
        if let Some(index) = found {
            match target {
                Pane::Repository => self.select_repository(index),
                Pane::Commit => self.select_commit(index),
                Pane::File => self.select_file(index),
                Pane::Diff => {
                    self.diff_match = Some(index);
                    self.ensure_diff_line_visible(index);
                }
            }
            let length = self.dataset_len(target);
            let wrapped = if (!backward && index <= current) || (backward && index >= current) {
                " (wrapped)"
            } else {
                ""
            };
            self.status = if initial {
                format!(
                    "Match for '{}' in {} ({}/{length}){wrapped}",
                    query.original,
                    target.title(),
                    index + 1
                )
            } else {
                format!(
                    "{direction} match for '{}' in {} ({}/{length}){wrapped}",
                    query.original,
                    target.title(),
                    index + 1
                )
            };
        } else {
            self.status = format!("No match for '{}' in {}", query.original, target.title());
        }
    }

    fn find_match(&self, pane: Pane, current: usize, backward: bool) -> Option<usize> {
        let needle = self.last_search.as_ref()?.needle.as_str();
        find_wrapped_direction(
            self.dataset_len(pane),
            current,
            backward,
            |index| match pane {
                Pane::Repository => self.visible_repositories()[index]
                    .display_name()
                    .to_lowercase()
                    .contains(needle),
                Pane::Commit => self.current_commits()[index]
                    .label()
                    .to_lowercase()
                    .contains(needle),
                Pane::File => self.current_files()[index]
                    .path
                    .to_lowercase()
                    .contains(needle),
                Pane::Diff => self.current_diff_lines()[index]
                    .text
                    .to_lowercase()
                    .contains(needle),
            },
        )
    }

    fn focus_previous(&mut self, status: &'static str) {
        if let Some(previous) = self.focus.previous() {
            self.focus = previous;
            self.status = status.to_owned();
        } else {
            self.status = "Already at repository pane".to_owned();
        }
    }

    fn focus_next(&mut self, status: &'static str) {
        if matches!(self.inbox.source, InboxSource::Live { .. }) && self.focus == Pane::Commit {
            self.open_live_commit();
            return;
        }
        if let Some(next) = self.focus.next()
            && next <= self.deepest_meaningful_pane()
        {
            self.focus = next;
            self.status = status.to_owned();
            return;
        }
        self.status = "No child pane to open".to_owned();
    }

    fn move_active(&mut self, upward: bool, amount: usize) {
        match self.focus {
            Pane::Repository => {
                let selected = moved_index(
                    self.repositories.selected,
                    self.visible.len(),
                    upward,
                    amount,
                );
                self.select_repository(selected);
            }
            Pane::Commit => {
                let selected = moved_index(
                    self.commits.selected,
                    self.current_commits().len(),
                    upward,
                    amount,
                );
                self.select_commit(selected);
            }
            Pane::File => {
                let selected = moved_index(
                    self.files.selected,
                    self.current_files().len(),
                    upward,
                    amount,
                );
                self.select_file(selected);
            }
            Pane::Diff => {
                self.diff_row_offset = 0;
                self.diff_scroll = if upward {
                    self.diff_scroll.saturating_sub(amount)
                } else {
                    self.diff_scroll.saturating_add(amount)
                };
            }
        }
        self.status = if upward { "Moved up" } else { "Moved down" }.to_owned();
    }

    fn move_to_first(&mut self) {
        match self.focus {
            Pane::Repository => self.select_repository(0),
            Pane::Commit => self.select_commit(0),
            Pane::File => self.select_file(0),
            Pane::Diff => {
                self.diff_scroll = 0;
                self.diff_row_offset = 0;
            }
        }
        self.status = "Moved to first position".to_owned();
    }

    fn move_to_last(&mut self) {
        match self.focus {
            Pane::Repository => {
                self.select_repository(self.visible.len().saturating_sub(1));
            }
            Pane::Commit => {
                self.select_commit(self.current_commits().len().saturating_sub(1));
            }
            Pane::File => {
                self.select_file(self.current_files().len().saturating_sub(1));
            }
            Pane::Diff => {
                self.diff_scroll = self.max_diff_scroll();
                self.diff_row_offset = self.tail_diff_row_offset();
            }
        }
        self.status = "Moved to last position".to_owned();
    }

    fn half_page_step(&self) -> usize {
        (self.viewport_heights[self.focus.index()] / 2).max(1)
    }

    fn select_repository(&mut self, selected: usize) {
        if selected != self.repositories.selected {
            self.repositories.selected = selected;
            self.commits = ListPosition::default();
            self.files = ListPosition::default();
            self.diff_scroll = 0;
            self.diff_row_offset = 0;
            self.clear_diff_match();
            self.cancel_detail_if_selection_changed();
        }
    }

    fn select_commit(&mut self, selected: usize) {
        if selected != self.commits.selected {
            self.commits.selected = selected;
            self.files = ListPosition::default();
            self.diff_scroll = 0;
            self.diff_row_offset = 0;
            self.clear_diff_match();
            self.cancel_detail_if_selection_changed();
        }
    }

    fn select_file(&mut self, selected: usize) {
        if selected != self.files.selected {
            self.files.selected = selected;
            self.diff_scroll = 0;
            self.diff_row_offset = 0;
            self.clear_diff_match();
        }
    }

    fn normalize(&mut self) {
        normalize_list(
            &mut self.repositories,
            self.visible.len(),
            self.viewport_heights[Pane::Repository.index()],
        );

        let commit_len = self.current_commits().len();
        normalize_list(
            &mut self.commits,
            commit_len,
            self.viewport_heights[Pane::Commit.index()],
        );

        let file_len = self.current_files().len();
        normalize_list(
            &mut self.files,
            file_len,
            self.viewport_heights[Pane::File.index()],
        );

        self.diff_scroll = self.diff_scroll.min(self.max_diff_scroll());
        self.diff_row_offset = self
            .diff_row_offset
            .min(self.diff_line_rows(self.diff_scroll).saturating_sub(1));
        self.focus = self.focus.min(self.deepest_meaningful_pane());
    }

    fn deepest_meaningful_pane(&self) -> Pane {
        if self.visible.is_empty() || self.current_commits().is_empty() {
            Pane::Repository
        } else if matches!(self.inbox.source, InboxSource::Live { .. }) {
            match self.current_detail_state() {
                Some(DetailState::Loading | DetailState::Ready(_) | DetailState::Failed(_)) => {
                    Pane::Diff
                }
                Some(DetailState::NotRequested) | None => Pane::Commit,
            }
        } else if self.current_files().is_empty() {
            Pane::Commit
        } else {
            Pane::Diff
        }
    }

    fn max_diff_scroll(&self) -> usize {
        let capacity = self.diff_patch_capacity();
        let lines = self.current_diff_lines();
        let mut rows: usize = 0;
        for index in (0..lines.len()).rev() {
            rows = rows.saturating_add(self.diff_line_rows(index));
            if rows > capacity {
                return index.saturating_add(1).min(lines.len().saturating_sub(1));
            }
        }
        0
    }

    fn ensure_diff_line_visible(&mut self, index: usize) {
        let capacity = self.diff_patch_capacity();
        if index < self.diff_scroll {
            self.diff_scroll = index;
            self.diff_row_offset = 0;
            return;
        }
        let mut rows: usize = 0;
        for line in self.diff_scroll..=index {
            rows = rows.saturating_add(self.diff_line_rows(line));
        }
        if rows > capacity {
            self.diff_scroll = index;
            let mut rows = self.diff_line_rows(index);
            while self.diff_scroll > 0 {
                let previous = self.diff_line_rows(self.diff_scroll - 1);
                if rows.saturating_add(previous) > capacity {
                    break;
                }
                rows += previous;
                self.diff_scroll -= 1;
            }
        }
        self.diff_row_offset = 0;
    }

    fn tail_diff_row_offset(&self) -> usize {
        let capacity = self.diff_patch_capacity();
        let total_rows = (self.diff_scroll..self.current_diff_lines().len())
            .map(|index| self.diff_line_rows(index))
            .sum::<usize>();
        total_rows.saturating_sub(capacity)
    }

    fn clear_diff_match(&mut self) {
        self.diff_match = None;
        self.diff_row_offset = 0;
    }

    pub fn diff_match(&self) -> Option<usize> {
        self.diff_match
    }

    pub fn diff_row_offset(&self) -> usize {
        self.diff_row_offset
    }

    pub fn diff_gutter_width(&self) -> usize {
        let max_number = self
            .current_diff_lines()
            .iter()
            .flat_map(|line| [line.old_line, line.new_line])
            .flatten()
            .max()
            .unwrap_or(0);
        decimal_width(max_number)
            .saturating_mul(2)
            .saturating_add(3)
    }

    pub fn diff_content_width(&self) -> usize {
        self.diff_viewport_width
            .saturating_sub(self.diff_gutter_width())
            .max(1)
    }

    pub(crate) fn diff_viewport_width(&self) -> usize {
        self.diff_viewport_width.max(1)
    }

    pub fn diff_line_rows(&self, index: usize) -> usize {
        self.current_diff_lines().get(index).map_or(0, |line| {
            wrap_text(&line.text, self.diff_content_width()).len()
        })
    }

    pub(crate) fn diff_notice_texts(&self) -> Vec<String> {
        let mut notices = Vec::with_capacity(2);
        if let Some(DetailState::Ready(detail)) = self.current_detail_state()
            && (detail.omitted_files > 0 || detail.more_files_available)
        {
            notices.push("Only first 300 files shown".to_owned());
        }
        if let Some(file) = self.current_file()
            && let PatchContent::Capped {
                omitted_lines,
                omitted_bytes,
                reason: PatchCapReason::FileLimit,
                ..
            } = &file.patch
        {
            notices.push(format!(
                "Patch capped locally: {omitted_lines} lines / {} KiB omitted",
                omitted_bytes.div_ceil(1024)
            ));
        }
        notices
    }

    fn diff_patch_capacity(&self) -> usize {
        let notice_rows = self
            .diff_notice_texts()
            .iter()
            .map(|notice| wrap_text(notice, self.diff_viewport_width).len())
            .sum::<usize>();
        self.viewport_heights[Pane::Diff.index()]
            .saturating_sub(notice_rows)
            .max(1)
    }

    pub fn inbox(&self) -> &Inbox {
        &self.inbox
    }

    /// Kept as a compatibility accessor for the fixture-driven demo tests.
    pub fn fixture(&self) -> &Inbox {
        self.inbox()
    }

    pub fn current_repository(&self) -> Option<&Repository> {
        let visible = self.visible.get(self.repositories.selected)?;
        self.inbox.repositories.get(visible.inbox_index)
    }

    pub fn visible_repositories(&self) -> Vec<&Repository> {
        self.visible
            .iter()
            .filter_map(|visible| self.inbox.repositories.get(visible.inbox_index))
            .collect()
    }

    pub fn current_commits(&self) -> Vec<&Commit> {
        let Some(visible) = self.visible.get(self.repositories.selected) else {
            return Vec::new();
        };
        let Some(repository) = self.inbox.repositories.get(visible.inbox_index) else {
            return Vec::new();
        };
        visible
            .commit_indices
            .iter()
            .filter_map(|index| repository.commits.get(*index))
            .collect()
    }

    pub fn current_commit(&self) -> Option<&Commit> {
        self.current_commits().get(self.commits.selected).copied()
    }

    pub fn current_files(&self) -> &[FileChange] {
        if self.inbox.child_panes_available() {
            self.current_commit()
                .map_or(&[], |commit| commit.files.as_slice())
        } else {
            match self.current_detail_state() {
                Some(DetailState::Ready(detail)) => &detail.files,
                _ => &[],
            }
        }
    }

    pub fn current_file(&self) -> Option<&FileChange> {
        self.current_files().get(self.files.selected)
    }

    pub fn current_diff_lines(&self) -> &[DiffLine] {
        self.current_file().map_or(&[], |file| file.patch.lines())
    }

    pub fn mode(&self) -> Mode {
        self.mode
    }

    pub fn focus(&self) -> Pane {
        self.focus
    }

    pub fn selected(&self, pane: Pane) -> usize {
        match pane {
            Pane::Repository => self.repositories.selected,
            Pane::Commit => self.commits.selected,
            Pane::File => self.files.selected,
            Pane::Diff => self.diff_scroll,
        }
    }

    pub fn scroll(&self, pane: Pane) -> usize {
        match pane {
            Pane::Repository => self.repositories.scroll,
            Pane::Commit => self.commits.scroll,
            Pane::File => self.files.scroll,
            Pane::Diff => self.diff_scroll,
        }
    }

    pub fn position(&self, pane: Pane) -> (usize, usize) {
        let length = self.dataset_len(pane);
        if length == 0 {
            (0, 0)
        } else {
            (self.selected(pane).saturating_add(1).min(length), length)
        }
    }

    pub fn pending_g(&self) -> bool {
        self.pending_g
    }

    pub fn search_query(&self) -> &str {
        &self.search_query
    }

    pub fn review_progress(&self) -> (usize, usize) {
        let total = self
            .inbox
            .repositories
            .iter()
            .map(|repository| repository.commits.len())
            .sum();
        let reviewed = self
            .inbox
            .repositories
            .iter()
            .flat_map(|repository| {
                repository
                    .commits
                    .iter()
                    .map(move |commit| (repository, commit))
            })
            .filter(|(repository, commit)| self.is_reviewed(repository.identity.id, &commit.sha))
            .count();
        (reviewed, total)
    }

    pub fn status(&self) -> &str {
        &self.status
    }

    pub fn should_quit(&self) -> bool {
        self.should_quit
    }

    pub fn live_state(&self) -> Option<&LiveInboxState> {
        self.live.as_ref()
    }

    pub fn current_detail_state(&self) -> Option<&DetailState> {
        self.detail_cache.get(&self.current_detail_key()?)
    }

    pub fn is_reviewed(&self, repository_id: u64, sha: &str) -> bool {
        ReviewKey::new(repository_id, sha)
            .ok()
            .is_some_and(|key| self.review_marks.is_reviewed(&key))
    }

    pub fn remaining_only(&self) -> bool {
        self.remaining_only
    }

    pub fn all_reviewed_empty(&self) -> bool {
        self.remaining_only
            && self.visible.is_empty()
            && self
                .inbox
                .repositories
                .iter()
                .any(|repository| !repository.commits.is_empty())
    }

    pub fn review_warning(&self) -> Option<&str> {
        self.review_warning.as_deref()
    }

    pub fn draft_warning(&self) -> Option<&str> {
        self.draft_warning.as_deref()
    }

    pub fn draft_store_available(&self) -> bool {
        self.draft_store.is_some()
    }

    pub fn draft_count(&self) -> usize {
        self.drafts.len()
    }

    #[cfg(test)]
    pub fn detail_cache_len(&self) -> usize {
        self.detail_cache.len()
    }

    fn selection_identity(&self) -> SelectionIdentity {
        SelectionIdentity {
            repository_id: self
                .current_repository()
                .map(|repository| repository.identity.id),
            commit_sha: self.current_commit().map(|commit| commit.sha.clone()),
        }
    }

    fn rebuild_projection(&mut self, preferred: Option<SelectionIdentity>) {
        let visible = self
            .inbox
            .repositories
            .iter()
            .enumerate()
            .filter_map(|(inbox_index, repository)| {
                let commit_indices: Vec<_> = repository
                    .commits
                    .iter()
                    .enumerate()
                    .filter_map(|(commit_index, commit)| {
                        (!self.remaining_only
                            || !self.is_reviewed(repository.identity.id, &commit.sha))
                        .then_some(commit_index)
                    })
                    .collect();
                (!self.remaining_only || !commit_indices.is_empty()).then_some(VisibleRepository {
                    inbox_index,
                    commit_indices,
                })
            })
            .collect::<Vec<_>>();
        self.visible = visible;

        if let Some(preferred) = preferred {
            if let Some(repository_id) = preferred.repository_id
                && let Some(visible_index) = self.visible.iter().position(|visible| {
                    self.inbox.repositories[visible.inbox_index].identity.id == repository_id
                })
            {
                self.repositories.selected = visible_index;
            }
            if let Some(sha) = preferred.commit_sha.as_deref()
                && let Some(visible_repository) = self.visible.get(self.repositories.selected)
                && let Some(commit_index) =
                    visible_repository
                        .commit_indices
                        .iter()
                        .position(|raw_index| {
                            self.inbox.repositories[visible_repository.inbox_index].commits
                                [*raw_index]
                                .sha
                                == sha
                        })
            {
                self.commits.selected = commit_index;
            }
        }
        normalize_list(
            &mut self.repositories,
            self.visible.len(),
            self.viewport_heights[Pane::Repository.index()],
        );
        let commit_len = self.current_commits().len();
        normalize_list(
            &mut self.commits,
            commit_len,
            self.viewport_heights[Pane::Commit.index()],
        );
    }

    fn current_detail_key(&self) -> Option<DetailKey> {
        let repository = self.current_repository()?;
        let commit = self.current_commit()?;
        Some(DetailKey {
            repository_id: repository.identity.id,
            sha: commit.sha.clone(),
        })
    }

    fn current_detail_target(&self) -> Option<(DetailKey, RepositoryIdentity)> {
        let repository = self.current_repository()?;
        let commit = self.current_commit()?;
        let review_key = ReviewKey::new(repository.identity.id, &commit.sha).ok()?;
        let key = DetailKey {
            repository_id: repository.identity.id,
            sha: review_key.sha().to_owned(),
        };
        Some((key, repository.identity.clone()))
    }

    fn cancel_detail_if_selection_changed(&mut self) {
        let selected = self.current_detail_key();
        if self
            .active_request
            .as_ref()
            .is_some_and(|active| Some(&active.key) != selected.as_ref())
        {
            self.cancel_active_detail();
        }
    }

    fn cancel_active_detail(&mut self) {
        let Some(active) = self.active_request.take() else {
            return;
        };
        if matches!(
            self.detail_cache.get(&active.key),
            Some(DetailState::Loading)
        ) {
            self.detail_cache.remove(&active.key);
            self.remove_detail_lru(&active.key);
        }
        self.effects.push_back(DetailEffect::Cancel {
            request_id: active.request_id,
        });
    }

    fn touch_detail(&mut self, key: &DetailKey) {
        self.remove_detail_lru(key);
        self.detail_lru.push_back(key.clone());
        while self.detail_cache.len() > DETAIL_CACHE_CAPACITY {
            let active_key = self.active_request.as_ref().map(|active| &active.key);
            let Some(position) = self
                .detail_lru
                .iter()
                .position(|candidate| Some(candidate) != active_key)
            else {
                break;
            };
            if let Some(evicted) = self.detail_lru.remove(position) {
                self.detail_cache.remove(&evicted);
            }
        }
    }

    fn remove_detail_lru(&mut self, key: &DetailKey) {
        if let Some(position) = self
            .detail_lru
            .iter()
            .position(|candidate| candidate == key)
        {
            self.detail_lru.remove(position);
        }
    }

    fn dataset_len(&self, pane: Pane) -> usize {
        match pane {
            Pane::Repository => self.visible.len(),
            Pane::Commit => self.current_commits().len(),
            Pane::File => self.current_files().len(),
            Pane::Diff => self.current_diff_lines().len(),
        }
    }
}

fn find_wrapped_direction(
    length: usize,
    current: usize,
    backward: bool,
    mut matches: impl FnMut(usize) -> bool,
) -> Option<usize> {
    if length == 0 {
        return None;
    }

    let current = current.min(length - 1);
    if backward {
        (0..current)
            .rev()
            .chain((current..length).rev())
            .find(|&index| matches(index))
    } else {
        ((current + 1)..length)
            .chain(0..=current)
            .find(|&index| matches(index))
    }
}

fn decimal_width(number: u32) -> usize {
    number.to_string().len().max(1)
}

fn moved_index(current: usize, length: usize, upward: bool, amount: usize) -> usize {
    if length == 0 {
        return 0;
    }
    if upward {
        current.saturating_sub(amount)
    } else {
        current.saturating_add(amount).min(length - 1)
    }
}

fn normalize_list(position: &mut ListPosition, length: usize, viewport_height: usize) {
    if length == 0 {
        *position = ListPosition::default();
        return;
    }

    position.selected = position.selected.min(length - 1);
    let capacity = viewport_height.max(1);
    let max_scroll = length.saturating_sub(capacity);
    if position.selected < position.scroll {
        position.scroll = position.selected;
    } else if position.selected >= position.scroll.saturating_add(capacity) {
        position.scroll = position.selected.saturating_add(1).saturating_sub(capacity);
    }
    position.scroll = position.scroll.min(max_scroll);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::comment_draft::{CommentDraft, CommentTarget, DraftStore, MemoryDraftStore};
    use crate::fixture::DemoFixture;
    use crate::github::{DetailFailure, FailureCategory, FailureScope};
    use crate::inbox::{ChildPane, FileStatus, GitHubAuthor, RepositoryIdentity};
    use crate::review_state::{FileReviewStore, ReviewStateError, ReviewStore};
    use chrono::{TimeZone, Utc};
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_STATE_TEST: AtomicU64 = AtomicU64::new(0);

    #[test]
    fn demo_app_loads_drafts_from_an_injected_memory_only_store() {
        let draft_store = MemoryDraftStore::default();
        let draft = CommentDraft::new(
            CommentTarget::commit(1, "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa").unwrap(),
            "Fictional draft",
        )
        .unwrap();
        draft_store.save(&draft).unwrap();

        let app = App::with_stores(
            DemoFixture::load(),
            Box::new(MemoryReviewStore::default()),
            Box::new(draft_store),
        );

        assert!(app.draft_store_available());
        assert_eq!(app.draft_count(), 1);
        assert!(app.draft_warning().is_none());
    }

    #[test]
    fn draft_load_failure_is_sanitized_and_disables_only_draft_persistence() {
        let app = App::with_store_results(
            DemoFixture::load(),
            Ok(Box::new(MemoryReviewStore::default())),
            Err(DraftStateError::Malformed),
        );

        assert!(!app.draft_store_available());
        assert_eq!(app.draft_count(), 0);
        assert!(app.review_warning().is_none());
        assert!(app.draft_warning().unwrap().contains("comment-drafts.json"));
    }

    fn files() -> Vec<FileChange> {
        ["one.rs", "two.rs", "three.rs"]
            .into_iter()
            .map(|path| FileChange {
                path: path.to_owned(),
                api_path_is_commentable: true,
                previous_path: None,
                status: FileStatus::Modified,
                additions: 0,
                deletions: 0,
                changes: 0,
                patch: crate::github::parse_patch_text("one\ntwo\nthree\nfour\nfive\nsix"),
            })
            .collect()
    }

    fn commit(sha: &str, subject: &str, files: Vec<FileChange>) -> Commit {
        Commit {
            sha: sha.to_owned(),
            subject: subject.to_owned(),
            author: GitHubAuthor {
                login: "fictional".to_owned(),
            },
            authored_at: Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap(),
            files: ChildPane::Available(files),
        }
    }

    fn commits() -> Vec<Commit> {
        vec![
            commit("1111111000000000000000000000000000000000", "one", files()),
            commit("2222222000000000000000000000000000000000", "two", files()),
            commit("3333333000000000000000000000000000000000", "three", files()),
        ]
    }

    fn repositories() -> Vec<Repository> {
        ["fictional/one", "fictional/two", "fictional/three"]
            .into_iter()
            .enumerate()
            .map(|(index, name)| Repository {
                identity: repository_identity(index as u64 + 1, name),
                commits: commits(),
            })
            .collect()
    }

    fn repository_identity(id: u64, display_name: &str) -> RepositoryIdentity {
        let (owner, name) = display_name.split_once('/').unwrap();
        RepositoryIdentity {
            id,
            owner: owner.to_owned(),
            name: name.to_owned(),
        }
    }

    fn app() -> App {
        let mut app = App::new(Inbox::demo(repositories()));
        app.viewport_heights = [2; 4];
        app
    }

    #[test]
    fn focus_moves_with_h_l_and_open_back() {
        let mut app = app();

        app.apply(Command::FocusNext);
        assert_eq!(app.focus(), Pane::Commit);
        app.apply(Command::Open);
        assert_eq!(app.focus(), Pane::File);
        app.apply(Command::FocusPrevious);
        assert_eq!(app.focus(), Pane::Commit);
        app.apply(Command::Open);
        app.apply(Command::Open);
        assert_eq!(app.focus(), Pane::Diff);
        app.apply(Command::Back);
        assert_eq!(app.focus(), Pane::File);
    }

    #[test]
    fn escape_at_root_does_not_quit() {
        let mut app = app();

        app.apply(Command::Back);

        assert_eq!(app.focus(), Pane::Repository);
        assert!(!app.should_quit());
    }

    #[test]
    fn list_movement_clamps_and_resets_children() {
        let mut app = app();
        app.apply(Command::FocusNext);
        app.apply(Command::MoveDown);
        app.apply(Command::FocusNext);
        app.apply(Command::MoveDown);
        assert_eq!(app.selected(Pane::Commit), 1);
        assert_eq!(app.selected(Pane::File), 1);

        app.focus = Pane::Repository;
        app.apply(Command::MoveDown);

        assert_eq!(app.selected(Pane::Repository), 1);
        assert_eq!(app.selected(Pane::Commit), 0);
        assert_eq!(app.selected(Pane::File), 0);
        assert_eq!(app.scroll(Pane::Diff), 0);

        for _ in 0..10 {
            app.apply(Command::MoveUp);
        }
        assert_eq!(app.selected(Pane::Repository), 0);
    }

    #[test]
    fn gg_and_uppercase_g_reach_boundaries() {
        let mut app = app();
        app.apply(Command::Last);
        assert_eq!(app.selected(Pane::Repository), 2);
        assert_eq!(app.scroll(Pane::Repository), 1);

        app.apply(Command::GPrefix);
        assert!(app.pending_g());
        app.apply(Command::GPrefix);

        assert_eq!(app.selected(Pane::Repository), 0);
        assert!(!app.pending_g());
    }

    #[test]
    fn unrelated_command_clears_g_before_normal_handling() {
        let mut app = app();

        app.apply(Command::GPrefix);
        app.apply(Command::MoveDown);
        assert_eq!(app.selected(Pane::Repository), 1);
        assert!(!app.pending_g());

        app.apply(Command::GPrefix);
        app.apply(Command::Unrelated);
        app.apply(Command::GPrefix);
        assert!(app.pending_g());
        assert_eq!(app.selected(Pane::Repository), 1);
    }

    #[test]
    fn half_page_uses_active_viewport_with_minimum_step() {
        let mut app = app();
        app.viewport_heights[Pane::Repository.index()] = 4;

        app.apply(Command::HalfPageDown);
        assert_eq!(app.selected(Pane::Repository), 2);
        app.apply(Command::HalfPageUp);
        assert_eq!(app.selected(Pane::Repository), 0);

        app.viewport_heights[Pane::Repository.index()] = 0;
        app.apply(Command::HalfPageDown);
        assert_eq!(app.selected(Pane::Repository), 1);
    }

    #[test]
    fn diff_navigation_and_boundaries_are_clamped() {
        let mut app = app();
        app.focus = Pane::Diff;

        app.apply(Command::Last);
        assert_eq!(app.scroll(Pane::Diff), 5);
        app.apply(Command::MoveDown);
        assert_eq!(app.scroll(Pane::Diff), 5);
        app.apply(Command::HalfPageUp);
        assert_eq!(app.scroll(Pane::Diff), 4);
        app.apply(Command::GPrefix);
        app.apply(Command::GPrefix);
        assert_eq!(app.scroll(Pane::Diff), 0);
        app.apply(Command::MoveUp);
        assert_eq!(app.scroll(Pane::Diff), 0);
    }

    #[test]
    fn resize_keeps_every_position_valid() {
        let mut app = App::new(DemoFixture::load());
        app.resize(100, 30);
        app.apply(Command::Last);
        app.apply(Command::FocusNext);
        app.apply(Command::Last);
        app.apply(Command::FocusNext);
        app.apply(Command::Last);
        app.apply(Command::FocusNext);
        app.apply(Command::Last);

        for (width, height) in [(1, 1), (60, 16), (100, 50), (0, 0)] {
            app.resize(width, height);
            assert!(app.selected(Pane::Repository) < app.inbox.repositories.len());
            assert!(app.selected(Pane::Commit) < app.current_commits().len());
            assert!(app.selected(Pane::File) < app.current_files().len());
            assert!(app.scroll(Pane::Diff) <= app.max_diff_scroll());
        }
    }

    #[test]
    fn empty_and_singleton_collections_never_underflow() {
        let one_repository = Inbox::demo(vec![Repository {
            identity: repository_identity(10, "fictional/only"),
            commits: Vec::new(),
        }]);
        let one_nested_repository = Inbox::demo(vec![Repository {
            identity: repository_identity(11, "fictional/only-nested"),
            commits: vec![commit(
                "0000001000000000000000000000000000000000",
                "only",
                vec![FileChange {
                    path: "only.rs".to_owned(),
                    api_path_is_commentable: true,
                    previous_path: None,
                    status: FileStatus::Modified,
                    additions: 0,
                    deletions: 0,
                    changes: 0,
                    patch: crate::github::parse_patch_text("only line"),
                }],
            )],
        }]);
        for inbox in [
            Inbox::demo(Vec::new()),
            one_repository,
            one_nested_repository,
        ] {
            let mut app = App::new(inbox);
            for command in [
                Command::MoveUp,
                Command::MoveDown,
                Command::HalfPageUp,
                Command::HalfPageDown,
                Command::Last,
                Command::GPrefix,
                Command::GPrefix,
                Command::Open,
                Command::Back,
            ] {
                app.apply(command);
                app.resize(0, 0);
            }

            assert_eq!(app.selected(Pane::Repository), 0);
            assert_eq!(app.scroll(Pane::Diff), 0);
            assert_eq!(app.focus(), Pane::Repository);
        }
    }

    #[test]
    fn live_inbox_open_starts_on_demand_detail_loading() {
        let selection = crate::day::select_day(
            crate::day::parse_date("2024-01-15").unwrap(),
            crate::day::parse_timezone("Etc/UTC").unwrap(),
            crate::day::TimezoneSource::Explicit,
        )
        .unwrap();
        let mut inbox = Inbox::live(selection);
        inbox.repositories = vec![Repository {
            identity: repository_identity(12, "owned/repository"),
            commits: commits(),
        }];
        let mut app = App::new(inbox);
        app.apply(Command::FocusNext);
        assert_eq!(app.focus(), Pane::Commit);
        app.apply(Command::FocusNext);
        assert_eq!(app.focus(), Pane::File);
        assert!(app.current_files().is_empty());
        assert!(matches!(
            app.current_detail_state(),
            Some(DetailState::Loading)
        ));
        assert!(matches!(
            app.take_detail_effects().as_slice(),
            [DetailEffect::Request { .. }]
        ));
    }

    #[test]
    fn normalization_repairs_all_out_of_range_positions() {
        let mut app = app();
        app.repositories = ListPosition {
            selected: usize::MAX,
            scroll: usize::MAX,
        };
        app.commits = app.repositories;
        app.files = app.repositories;
        app.diff_scroll = usize::MAX;
        app.focus = Pane::Diff;

        app.resize(60, 16);

        assert_eq!(app.selected(Pane::Repository), 2);
        assert_eq!(app.selected(Pane::Commit), 2);
        assert_eq!(app.selected(Pane::File), 2);
        assert!(app.scroll(Pane::Repository) <= app.selected(Pane::Repository));
        assert!(app.scroll(Pane::Commit) <= app.selected(Pane::Commit));
        assert!(app.scroll(Pane::File) <= app.selected(Pane::File));
        assert!(app.scroll(Pane::Diff) <= app.max_diff_scroll());
    }

    #[test]
    fn search_entry_isolates_printable_navigation_and_help_keys() {
        let mut app = app();
        let focus = app.focus();
        let selected = app.selected(Pane::Repository);

        app.handle_input(Input::Character('/'));
        for character in ['h', 'j', 'k', 'l', 'g', 'G', 'n', 'N', '?', 'q'] {
            app.handle_input(Input::Character(character));
        }

        assert_eq!(
            app.mode(),
            Mode::SearchEntry {
                target: Pane::Repository
            }
        );
        assert_eq!(app.search_query(), "hjklgGnN?q");
        assert_eq!(app.focus(), focus);
        assert_eq!(app.selected(Pane::Repository), selected);
        assert!(!app.should_quit());
    }

    #[test]
    fn search_query_supports_editing_cancel_and_empty_apply() {
        let mut app = app();
        app.handle_input(Input::Character('l'));
        let focus = app.focus();

        app.handle_input(Input::Character('/'));
        for character in ['t', 'w', 'x'] {
            app.handle_input(Input::Character(character));
        }
        app.handle_input(Input::Backspace);
        assert_eq!(app.search_query(), "tw");
        app.handle_input(Input::Escape);
        assert_eq!(app.mode(), Mode::Normal);
        assert_eq!(app.focus(), focus);
        assert!(app.search_query().is_empty());
        assert_eq!(app.status(), "Search cancelled");

        app.handle_input(Input::Character('/'));
        app.handle_input(Input::Enter);
        assert_eq!(app.mode(), Mode::Normal);
        assert_eq!(app.focus(), focus);
        assert_eq!(app.status(), "Empty search; position unchanged");
    }

    #[test]
    fn search_selects_list_matches_case_insensitively_and_wraps() {
        let mut app = app();

        enter_search(&mut app, "THREE");
        assert_eq!(app.selected(Pane::Repository), 2);
        assert!(app.status().contains("Match for 'THREE'"));

        enter_search(&mut app, "one");
        assert_eq!(app.selected(Pane::Repository), 0);
        assert!(app.status().contains("(1/3)"));

        app.handle_input(Input::Character('l'));
        enter_search(&mut app, "3333333");
        assert_eq!(app.selected(Pane::Commit), 2);
        app.handle_input(Input::Character('l'));
        enter_search(&mut app, "TWO.RS");
        assert_eq!(app.selected(Pane::File), 1);
    }

    #[test]
    fn diff_search_scrolls_to_match_wraps_and_preserves_no_match_state() {
        let mut app = app();
        app.focus = Pane::Diff;

        enter_search(&mut app, "five");
        assert_eq!(app.scroll(Pane::Diff), 4);
        enter_search(&mut app, "one");
        assert_eq!(app.scroll(Pane::Diff), 0);

        let before = app.scroll(Pane::Diff);
        enter_search(&mut app, "not present");
        assert_eq!(app.scroll(Pane::Diff), before);
        assert_eq!(app.status(), "No match for 'not present' in Diff");
    }

    #[test]
    fn repeated_search_wraps_in_every_pane_and_reports_missing_context() {
        let mut app = app();
        app.handle_input(Input::Character('n'));
        assert_eq!(app.status(), "No previous search");

        enter_search(&mut app, "fictional");
        assert_eq!(app.selected(Pane::Repository), 1);
        app.handle_input(Input::Character('n'));
        assert_eq!(app.selected(Pane::Repository), 2);
        app.handle_input(Input::Character('n'));
        assert_eq!(app.selected(Pane::Repository), 0);
        assert!(app.status().contains("wrapped"));
        app.handle_input(Input::Character('N'));
        assert_eq!(app.selected(Pane::Repository), 2);

        app.handle_input(Input::Character('l'));
        enter_search(&mut app, "e");
        assert_eq!(app.selected(Pane::Commit), 2);
        app.handle_input(Input::Character('n'));
        assert_eq!(app.selected(Pane::Commit), 0);
        app.handle_input(Input::Character('N'));
        assert_eq!(app.selected(Pane::Commit), 2);

        app.handle_input(Input::Character('l'));
        enter_search(&mut app, ".rs");
        assert_eq!(app.selected(Pane::File), 1);
        app.handle_input(Input::Character('n'));
        assert_eq!(app.selected(Pane::File), 2);
        app.handle_input(Input::Character('N'));
        assert_eq!(app.selected(Pane::File), 1);

        app.handle_input(Input::Character('l'));
        enter_search(&mut app, "e");
        assert_eq!(app.diff_match(), Some(2));
        app.handle_input(Input::Character('n'));
        assert_eq!(app.diff_match(), Some(4));
        app.handle_input(Input::Character('n'));
        assert_eq!(app.diff_match(), Some(0));
        app.handle_input(Input::Character('N'));
        assert_eq!(app.diff_match(), Some(4));

        app.select_file(2);
        assert_eq!(app.diff_match(), None);
        app.focus = Pane::Diff;
        enter_search(&mut app, "e");
        assert!(app.diff_match().is_some());
        app.select_commit(0);
        assert_eq!(app.diff_match(), None);
        app.focus = Pane::Diff;
        enter_search(&mut app, "e");
        app.select_repository(0);
        assert_eq!(app.diff_match(), None);
        app.focus = Pane::Diff;
        enter_search(&mut app, "e");
        assert!(app.diff_match().is_some());
        app.toggle_remaining();
        assert_eq!(app.diff_match(), None);
    }

    #[test]
    fn incoming_live_snapshot_invalidates_a_current_diff_match() {
        let repository = Repository {
            identity: repository_identity(77, "owned/live"),
            commits: commits(),
        };
        let mut app = live_app_with(vec![repository.clone()]);
        let key = DetailKey {
            repository_id: 77,
            sha: app.current_commit().unwrap().sha.clone(),
        };
        app.detail_cache.insert(
            key,
            DetailState::Ready(Arc::new(CommitDetail {
                files: files(),
                omitted_files: 0,
                more_files_available: false,
            })),
        );
        app.focus = Pane::Diff;
        enter_search(&mut app, "e");
        assert!(app.diff_match().is_some());
        app.apply_load_event(LoadEvent::RepositorySnapshot {
            repository_index: 0,
            repository: LoadedRepository {
                repository,
                branch_count: 1,
                coverage: RepositoryCoverage::Complete,
            },
        });
        assert_eq!(app.diff_match(), None);
    }

    #[test]
    fn help_ignores_normal_commands_then_escape_restores_focus() {
        let mut app = app();
        app.handle_input(Input::Character('l'));
        let focus = app.focus();
        let selected = app.selected(Pane::Commit);

        app.handle_input(Input::Character('?'));
        assert_eq!(
            app.mode(),
            Mode::Help {
                previous_focus: focus
            }
        );
        for input in [
            Input::Character('h'),
            Input::Character('j'),
            Input::Character('/'),
            Input::Character('q'),
            Input::Enter,
        ] {
            app.handle_input(input);
        }
        assert_eq!(app.focus(), focus);
        assert_eq!(app.selected(Pane::Commit), selected);
        assert!(!app.should_quit());

        app.handle_input(Input::Escape);
        assert_eq!(app.mode(), Mode::Normal);
        assert_eq!(app.focus(), focus);
        app.handle_input(Input::Character('j'));
        assert_eq!(app.selected(Pane::Commit), selected + 1);
    }

    #[test]
    fn resize_does_not_retarget_an_active_search() {
        let mut app = app();
        app.focus = Pane::File;
        app.handle_input(Input::Character('/'));

        for (width, height) in [(1, 1), (0, 0), (120, 40)] {
            app.resize(width, height);
            assert_eq!(app.mode(), Mode::SearchEntry { target: Pane::File });
            assert_eq!(app.focus(), Pane::File);
        }
    }

    fn enter_search(app: &mut App, query: &str) {
        app.handle_input(Input::Character('/'));
        for character in query.chars() {
            app.handle_input(Input::Character(character));
        }
        app.handle_input(Input::Enter);
    }

    #[test]
    fn quit_is_explicit() {
        let mut app = app();
        app.apply(Command::Quit);
        assert!(app.should_quit());
    }

    fn live_app() -> App {
        let selection = crate::day::select_day(
            crate::day::parse_date("2024-01-15").unwrap(),
            crate::day::parse_timezone("Etc/UTC").unwrap(),
            crate::day::TimezoneSource::Explicit,
        )
        .unwrap();
        App::new(Inbox::live(selection))
    }

    fn live_app_with(repositories: Vec<Repository>) -> App {
        let mut app = live_app();
        for (repository_index, repository) in repositories.into_iter().enumerate() {
            app.apply_load_event(LoadEvent::RepositorySnapshot {
                repository_index,
                repository: LoadedRepository {
                    repository,
                    branch_count: 1,
                    coverage: RepositoryCoverage::Complete,
                },
            });
        }
        app
    }

    fn detail() -> CommitDetail {
        CommitDetail {
            files: files(),
            omitted_files: 0,
            more_files_available: false,
        }
    }

    fn take_request(app: &mut App) -> (u64, DetailKey) {
        let effects = app.take_detail_effects();
        let [
            DetailEffect::Request {
                request_id, key, ..
            },
        ] = effects.as_slice()
        else {
            panic!("expected exactly one detail request, got {effects:?}");
        };
        (*request_id, key.clone())
    }

    fn detail_failure(category: FailureCategory) -> DetailFailure {
        DetailFailure::Load(LoadFailure {
            category,
            scope: FailureScope::CommitDetail,
            http_status: None,
        })
    }

    #[derive(Debug)]
    struct FailingReviewStore;

    impl ReviewStore for FailingReviewStore {
        fn load(&self) -> Result<ReviewMarks, ReviewStateError> {
            Ok(ReviewMarks::default())
        }

        fn set_reviewed(
            &self,
            _key: &ReviewKey,
            _reviewed: bool,
        ) -> Result<ReviewMarks, ReviewStateError> {
            Err(ReviewStateError::Write(
                std::io::ErrorKind::PermissionDenied,
            ))
        }
    }

    fn state_test_path() -> PathBuf {
        let sequence = NEXT_STATE_TEST.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "reviewbox-app-state-{}-{sequence}.json",
            std::process::id()
        ))
    }

    fn loaded_repository(name: &str, commits: Vec<Commit>) -> LoadedRepository {
        let id = name.bytes().map(u64::from).sum();
        LoadedRepository {
            repository: Repository {
                identity: repository_identity(id, name),
                commits,
            },
            branch_count: 2,
            coverage: RepositoryCoverage::Complete,
        }
    }

    #[test]
    fn loader_snapshots_preserve_repository_and_full_sha_selection() {
        let mut app = live_app();
        let alpha = loaded_repository("fixture/alpha", commits());
        let beta = loaded_repository("fixture/beta", commits());
        app.apply_load_event(LoadEvent::RepositorySnapshot {
            repository_index: 0,
            repository: alpha.clone(),
        });
        app.apply_load_event(LoadEvent::RepositorySnapshot {
            repository_index: 1,
            repository: beta.clone(),
        });
        app.apply(Command::MoveDown);
        app.apply(Command::Open);
        app.apply(Command::MoveDown);
        let selected_sha = app.current_commit().unwrap().sha.clone();

        let mut updated_alpha = alpha;
        updated_alpha.repository.commits.reverse();
        app.apply_load_event(LoadEvent::RepositorySnapshot {
            repository_index: 0,
            repository: updated_alpha,
        });
        let mut updated_beta = beta;
        updated_beta.repository.commits.insert(
            0,
            commit(
                "9999999000000000000000000000000000000000",
                "newest",
                Vec::new(),
            ),
        );
        app.apply_load_event(LoadEvent::RepositorySnapshot {
            repository_index: 1,
            repository: updated_beta,
        });

        assert_eq!(
            app.current_repository().unwrap().display_name(),
            "fixture/beta"
        );
        assert_eq!(app.current_commit().unwrap().sha, selected_sha);
        assert_eq!(app.selected(Pane::Commit), 2);
    }

    #[test]
    fn loader_mutations_clamp_positions_and_bound_sanitized_failures() {
        let mut app = live_app();
        app.apply_load_event(LoadEvent::RepositorySnapshot {
            repository_index: 0,
            repository: loaded_repository("fixture/alpha", commits()),
        });
        app.apply(Command::Open);
        app.apply(Command::Last);
        assert_eq!(app.selected(Pane::Commit), 2);

        app.apply_load_event(LoadEvent::RepositorySnapshot {
            repository_index: 0,
            repository: loaded_repository(
                "fixture/alpha",
                vec![commit(
                    "1111111000000000000000000000000000000000",
                    "one",
                    Vec::new(),
                )],
            ),
        });
        for branch_index in 0..5 {
            app.apply_load_event(LoadEvent::Failure(LoadFailure {
                category: crate::github::FailureCategory::PermissionOrNotFound,
                scope: crate::github::FailureScope::Branch {
                    repository_index: 0,
                    branch_index,
                },
                http_status: Some(403),
            }));
        }

        assert_eq!(app.selected(Pane::Commit), 0);
        assert_eq!(app.live_state().unwrap().failures().len(), 3);
        assert_eq!(app.live_state().unwrap().omitted_failures(), 2);
    }

    #[test]
    fn live_detail_open_is_single_inflight_and_success_populates_files() {
        let mut app = live_app_with(vec![Repository {
            identity: repository_identity(41, "fixture/details"),
            commits: commits(),
        }]);
        app.apply(Command::Open);
        app.apply(Command::Open);
        let (request_id, key) = take_request(&mut app);

        app.apply(Command::Back);
        app.apply(Command::Open);
        assert!(app.take_detail_effects().is_empty());

        app.apply_detail_result(DetailResult {
            request_id,
            key,
            outcome: Ok(detail()),
        });
        assert_eq!(app.current_files().len(), 3);
        assert!(matches!(
            app.current_detail_state(),
            Some(DetailState::Ready(_))
        ));
    }

    #[test]
    fn switching_commits_cancels_loading_and_stale_results_are_ignored() {
        let mut app = live_app_with(vec![Repository {
            identity: repository_identity(42, "fixture/stale"),
            commits: commits(),
        }]);
        app.apply(Command::Open);
        app.apply(Command::Open);
        let (old_id, old_key) = take_request(&mut app);
        app.apply(Command::Back);
        app.apply(Command::MoveDown);
        let effects = app.take_detail_effects();
        assert!(matches!(
            effects.as_slice(),
            [DetailEffect::Cancel { request_id }] if *request_id == old_id
        ));

        app.apply(Command::Open);
        let (new_id, new_key) = take_request(&mut app);
        app.apply_detail_result(DetailResult {
            request_id: old_id,
            key: old_key.clone(),
            outcome: Ok(detail()),
        });
        assert!(!app.detail_cache.contains_key(&old_key));
        assert!(matches!(
            app.detail_cache.get(&new_key),
            Some(DetailState::Loading)
        ));

        app.apply_detail_result(DetailResult {
            request_id: new_id,
            key: new_key,
            outcome: Ok(detail()),
        });
        assert_eq!(app.current_files().len(), 3);
    }

    #[test]
    fn failed_details_can_retry_and_current_cancellation_returns_to_not_requested() {
        let mut app = live_app_with(vec![Repository {
            identity: repository_identity(43, "fixture/retry"),
            commits: commits(),
        }]);
        app.apply(Command::Open);
        app.apply(Command::Open);
        let (failed_id, key) = take_request(&mut app);
        app.apply_detail_result(DetailResult {
            request_id: failed_id,
            key: key.clone(),
            outcome: Err(DetailFailure::ResponseTruncated),
        });
        assert!(matches!(
            app.current_detail_state(),
            Some(DetailState::Failed(_))
        ));

        app.apply(Command::Back);
        app.apply(Command::Open);
        let (retry_id, retry_key) = take_request(&mut app);
        assert_ne!(retry_id, failed_id);
        app.apply_detail_result(DetailResult {
            request_id: retry_id,
            key: retry_key,
            outcome: Err(detail_failure(FailureCategory::Cancelled)),
        });
        assert!(app.current_detail_state().is_none());
        assert_eq!(app.focus(), Pane::Commit);
    }

    #[test]
    fn detail_cache_evicts_least_recent_entries_beyond_sixteen() {
        let many_commits = (0..17)
            .map(|index| {
                commit(
                    &format!("{index:040x}"),
                    &format!("commit {index}"),
                    Vec::new(),
                )
            })
            .collect();
        let mut app = live_app_with(vec![Repository {
            identity: repository_identity(44, "fixture/cache"),
            commits: many_commits,
        }]);
        app.apply(Command::Open);
        let first_key = app.current_detail_key().unwrap();
        for index in 0..17 {
            app.select_commit(index);
            app.apply(Command::Open);
            let (request_id, key) = take_request(&mut app);
            app.apply_detail_result(DetailResult {
                request_id,
                key,
                outcome: Ok(detail()),
            });
            app.apply(Command::Back);
        }

        assert_eq!(app.detail_cache_len(), DETAIL_CACHE_CAPACITY);
        assert!(!app.detail_cache.contains_key(&first_key));
    }

    #[test]
    fn review_toggle_persists_in_memory_and_failed_save_keeps_mark_unchanged() {
        let repository = Repository {
            identity: repository_identity(51, "fixture/marks"),
            commits: commits(),
        };
        let mut app = live_app_with(vec![repository.clone()]);
        app.apply(Command::Open);
        app.apply(Command::ToggleReviewed);
        let commit = app.current_commit().unwrap();
        assert!(app.is_reviewed(51, &commit.sha));
        assert_eq!(app.status(), "Marked commit reviewed");
        app.apply(Command::ToggleReviewed);
        assert!(!app.is_reviewed(51, &app.current_commit().unwrap().sha));

        let mut failed = App::with_review_store(
            {
                let mut inbox = app.inbox.clone();
                inbox.repositories = vec![repository];
                inbox
            },
            Box::new(FailingReviewStore),
        );
        failed.rebuild_projection(None);
        failed.apply(Command::Open);
        failed.apply(Command::ToggleReviewed);
        assert!(!failed.is_reviewed(51, &failed.current_commit().unwrap().sha));
        assert!(failed.status().contains("mark unchanged"));
    }

    #[test]
    fn unavailable_store_rejects_marks_without_creating_demo_state() {
        let mut inbox = live_app().inbox;
        inbox.repositories = vec![Repository {
            identity: repository_identity(52, "fixture/unavailable"),
            commits: commits(),
        }];
        let mut app = App::with_review_store_result(inbox, Err(ReviewStateError::Unavailable));
        app.apply(Command::Open);
        app.apply(Command::ToggleReviewed);

        assert!(app.review_warning().unwrap().contains("unavailable"));
        assert!(app.status().contains("mark unchanged"));
        assert!(!app.is_reviewed(52, &app.current_commit().unwrap().sha));
    }

    #[test]
    fn remaining_filter_hides_reviewed_commits_and_empty_repositories() {
        let mut app = live_app_with(vec![
            Repository {
                identity: repository_identity(61, "fixture/first"),
                commits: vec![commits().remove(0)],
            },
            Repository {
                identity: repository_identity(62, "fixture/second"),
                commits: vec![commits().remove(1)],
            },
        ]);
        app.apply(Command::Open);
        app.apply(Command::ToggleReviewed);
        app.apply(Command::ToggleRemaining);

        assert_eq!(app.visible_repositories().len(), 1);
        assert_eq!(app.current_repository().unwrap().identity.id, 62);
        assert_eq!(app.current_commits().len(), 1);

        app.apply(Command::ToggleReviewed);
        assert!(app.all_reviewed_empty());
        assert!(app.visible_repositories().is_empty());
        assert_eq!(app.focus(), Pane::Repository);
    }

    #[test]
    fn filtered_selection_survives_incremental_snapshot_reordering_by_identity() {
        let mut app = live_app();
        let mut repository = loaded_repository("fixture/reorder", commits());
        let repository_id = repository.repository.identity.id;
        app.apply_load_event(LoadEvent::RepositorySnapshot {
            repository_index: 0,
            repository: repository.clone(),
        });
        app.apply(Command::Open);
        app.apply(Command::ToggleReviewed);
        app.apply(Command::ToggleRemaining);
        app.apply(Command::MoveDown);
        let selected_sha = app.current_commit().unwrap().sha.clone();

        repository.repository.commits.reverse();
        app.apply_load_event(LoadEvent::RepositorySnapshot {
            repository_index: 0,
            repository,
        });

        assert_eq!(app.current_repository().unwrap().identity.id, repository_id);
        assert_eq!(app.current_commit().unwrap().sha, selected_sha);
    }

    #[test]
    fn file_store_marks_are_loaded_by_a_fresh_app_instance() {
        let path = state_test_path();
        let repository = Repository {
            identity: repository_identity(71, "fixture/restart"),
            commits: commits(),
        };
        let mut inbox = live_app().inbox;
        inbox.repositories = vec![repository.clone()];
        let mut first =
            App::with_review_store(inbox, Box::new(FileReviewStore::at(&path).unwrap()));
        first.apply(Command::Open);
        first.apply(Command::ToggleReviewed);
        assert!(path.exists());

        let mut restarted = App::with_review_store(
            live_app().inbox,
            Box::new(FileReviewStore::at(&path).unwrap()),
        );
        restarted.apply(Command::ToggleRemaining);
        restarted.apply_load_event(LoadEvent::RepositorySnapshot {
            repository_index: 0,
            repository: LoadedRepository {
                repository,
                branch_count: 1,
                coverage: RepositoryCoverage::Complete,
            },
        });
        assert_eq!(restarted.current_commits().len(), 2);
        assert!(
            !restarted
                .current_commits()
                .iter()
                .any(|commit| commit.sha.starts_with("1111111"))
        );

        let _ = fs::remove_file(&path);
    }
}
