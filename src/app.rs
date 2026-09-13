use std::collections::{BTreeMap, HashMap, VecDeque};
use std::fs::File;
use std::io::Read;
use std::sync::Arc;

use ratatui::layout::Rect;

use crate::comment_draft::{
    CommentAnchor, CommentDraft, CommentDrafts, CommentTarget, DraftStateError, DraftStore,
    MAX_DRAFT_CHARACTERS, MemoryDraftStore, SubmissionAttempt, line_target,
};
use crate::external_editor::EditorOutcome;
use crate::github::{
    CommentFailure, DetailFailure, DetailState, ExistingCommentAnchor, ExistingComments,
    FailureCategory, LoadEvent, LoadFailure, LoadProgress, LoadStatus, LoadedRepository,
    PublishOutcome, RepositoryCoverage,
};
use crate::inbox::{
    Commit, CommitDetail, DiffLine, DiffLineKind, FileChange, Inbox, InboxSource, PatchCapReason,
    PatchContent, Repository, RepositoryIdentity,
};
use crate::review_state::{
    MemoryReviewStore, ReviewKey, ReviewMarks, ReviewStateError, ReviewStore,
};
use crate::ui_layout::{ReviewPaneLayout, wrap_text};

pub const MIN_FULL_WIDTH: u16 = 60;
pub const MIN_FULL_HEIGHT: u16 = 16;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mode {
    Normal,
    SearchEntry {
        target: Pane,
    },
    Help {
        previous_focus: Pane,
    },
    Edit {
        target: CommentTarget,
        previous_focus: Pane,
    },
    Comments {
        previous_focus: Pane,
    },
    ConfirmPublish {
        target: CommentTarget,
        previous_focus: Pane,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditBuffer {
    text: String,
    cursor: usize,
    saved: Option<String>,
}

impl EditBuffer {
    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn cursor(&self) -> usize {
        self.cursor
    }

    pub fn is_saved(&self) -> bool {
        self.saved.is_some()
    }
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
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    Cancel,
    ExternalEditor,
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
        keys: "c",
        action: "edit commit or selected-line draft",
    },
    HelpBinding {
        keys: "E",
        action: "edit draft with $VISUAL / $EDITOR",
    },
    HelpBinding {
        keys: "P",
        action: "publish draft after confirmation",
    },
    HelpBinding {
        keys: "C",
        action: "open existing commit comments",
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
    HelpBinding {
        keys: "Edit: printable / Enter",
        action: "insert text / newline",
    },
    HelpBinding {
        keys: "Edit: arrows / Home / End",
        action: "move the text cursor",
    },
    HelpBinding {
        keys: "Edit: Esc / Ctrl-g",
        action: "save and return / cancel",
    },
    HelpBinding {
        keys: "Edit: Ctrl-e",
        action: "edit current buffer externally",
    },
    HelpBinding {
        keys: "Edit: Ctrl-c",
        action: "save and quit",
    },
    HelpBinding {
        keys: "Comments: j/k / r / Esc",
        action: "scroll / refresh / close",
    },
    HelpBinding {
        keys: "Publish: y / any other key",
        action: "confirm / cancel",
    },
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EditorOrigin {
    Normal,
    Edit,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EditorRequest {
    pub target: CommentTarget,
    pub body: String,
    pub origin: EditorOrigin,
}

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommentListState {
    Loading,
    Loaded(ExistingComments),
    Failed(CommentFailure),
}

#[derive(Debug, Clone)]
pub enum CommentEffect {
    Load {
        request_id: u64,
        key: DetailKey,
        repository: RepositoryIdentity,
        marker: Option<String>,
        reconcile_target: Option<CommentTarget>,
    },
    Publish {
        request_id: u64,
        key: DetailKey,
        target: CommentTarget,
        repository: RepositoryIdentity,
        body_with_marker: String,
    },
    Cancel {
        request_id: u64,
    },
}

#[derive(Debug)]
pub enum CommentResultOutcome {
    Loaded(Result<ExistingComments, CommentFailure>),
    Published(PublishOutcome),
}

#[derive(Debug)]
pub struct CommentResult {
    pub request_id: u64,
    pub key: DetailKey,
    pub target: Option<CommentTarget>,
    pub outcome: CommentResultOutcome,
}

#[derive(Debug, Clone)]
struct ActiveCommentLoad {
    request_id: u64,
    key: DetailKey,
}

#[derive(Debug, Clone)]
struct ActivePublish {
    request_id: u64,
    key: DetailKey,
    target: CommentTarget,
    repository: RepositoryIdentity,
}

pub trait AttemptSource: std::fmt::Debug {
    fn next(&self) -> Result<SubmissionAttempt, ()>;
    fn now(&self) -> chrono::DateTime<chrono::Utc>;
}

#[derive(Debug)]
pub struct SystemAttemptSource;

impl AttemptSource for SystemAttemptSource {
    fn next(&self) -> Result<SubmissionAttempt, ()> {
        let mut bytes = [0_u8; 16];
        File::open("/dev/urandom")
            .and_then(|mut file| file.read_exact(&mut bytes))
            .map_err(|_| ())?;
        let attempt = bytes
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        SubmissionAttempt::new(attempt, chrono::Utc::now()).map_err(|_| ())
    }
    fn now(&self) -> chrono::DateTime<chrono::Utc> {
        chrono::Utc::now()
    }
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
    diff_cursor: usize,
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
    edit_buffer: Option<EditBuffer>,
    remaining_only: bool,
    visible: Vec<VisibleRepository>,
    detail_cache: HashMap<DetailKey, DetailState>,
    detail_lru: VecDeque<DetailKey>,
    active_request: Option<ActiveDetailRequest>,
    next_request_id: u64,
    effects: VecDeque<DetailEffect>,
    editor_requests: VecDeque<EditorRequest>,
    comment_cache: HashMap<DetailKey, CommentListState>,
    active_comment_load: Option<ActiveCommentLoad>,
    active_publish: Option<ActivePublish>,
    comment_effects: VecDeque<CommentEffect>,
    comments_scroll: usize,
    attempt_source: Box<dyn AttemptSource>,
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
        let mut status = match &inbox.source {
            InboxSource::Demo => "Offline fictional demo".to_owned(),
            InboxSource::Live { .. } => "GitHub loading started".to_owned(),
        };
        if drafts.iter().any(|draft| draft.submission.is_some()) {
            status = "A comment publish is unverified; press P or open C to reconcile".to_owned();
        }
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
            diff_cursor: 0,
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
            edit_buffer: None,
            remaining_only: false,
            visible: Vec::new(),
            detail_cache: HashMap::new(),
            detail_lru: VecDeque::new(),
            active_request: None,
            next_request_id: 1,
            effects: VecDeque::new(),
            editor_requests: VecDeque::new(),
            comment_cache: HashMap::new(),
            active_comment_load: None,
            active_publish: None,
            comment_effects: VecDeque::new(),
            comments_scroll: 0,
            attempt_source: Box::new(SystemAttemptSource),
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
        let previous_detail = self.current_detail_key();

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
        if previous_detail != self.current_detail_key() {
            self.reset_diff_position();
        }
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
        match &self.mode {
            Mode::Normal => self.apply_normal(command),
            Mode::SearchEntry { .. }
            | Mode::Help { .. }
            | Mode::Edit { .. }
            | Mode::Comments { .. }
            | Mode::ConfirmPublish { .. } => {}
        }
        self.normalize();
    }

    pub fn handle_input(&mut self, input: Input) {
        if input == Input::Quit {
            self.pending_g = false;
            if matches!(self.mode, Mode::Edit { .. }) {
                if self.persist_edit() {
                    self.quit();
                }
            } else {
                self.apply_normal(Command::Quit);
            }
            return;
        }

        match self.mode.clone() {
            Mode::Normal => self.handle_normal_input(input),
            Mode::SearchEntry { target } => self.handle_search_input(target, input),
            Mode::Help { previous_focus } => self.handle_help_input(previous_focus, input),
            Mode::Edit {
                target: _,
                previous_focus,
            } => self.handle_edit_input(previous_focus, input),
            Mode::Comments { previous_focus } => self.handle_comments_input(previous_focus, input),
            Mode::ConfirmPublish {
                target,
                previous_focus,
            } => self.handle_publish_confirmation(target, previous_focus, input),
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
            Input::Character('c') => {
                self.open_comment_editor();
                return;
            }
            Input::Character('E') => {
                self.request_external_editor_from_normal();
                return;
            }
            Input::Character('P') => {
                self.request_publish();
                return;
            }
            Input::Character('C') => {
                self.open_comments();
                return;
            }
            Input::Character('n') => Command::SearchNext,
            Input::Character('N') => Command::SearchPrevious,
            Input::Enter => Command::Open,
            Input::Escape => Command::Back,
            Input::HalfPageDown => Command::HalfPageDown,
            Input::HalfPageUp => Command::HalfPageUp,
            Input::Character(_)
            | Input::Backspace
            | Input::Left
            | Input::Right
            | Input::Up
            | Input::Down
            | Input::Home
            | Input::End
            | Input::Cancel
            | Input::ExternalEditor
            | Input::Unrelated
            | Input::Quit => Command::Unrelated,
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
            Input::Left
            | Input::Right
            | Input::Up
            | Input::Down
            | Input::Home
            | Input::End
            | Input::Cancel
            | Input::ExternalEditor
            | Input::HalfPageDown
            | Input::HalfPageUp
            | Input::Unrelated
            | Input::Quit => {}
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

    fn handle_comments_input(&mut self, previous_focus: Pane, input: Input) {
        match input {
            Input::Character('j') | Input::Down => {
                self.comments_scroll = self.comments_scroll.saturating_add(1)
            }
            Input::Character('k') | Input::Up => {
                self.comments_scroll = self.comments_scroll.saturating_sub(1)
            }
            Input::Character('r') => self.load_current_comments(true),
            Input::Escape => {
                self.focus = previous_focus;
                self.mode = Mode::Normal;
                self.status = "Comments closed".to_owned();
            }
            _ => {}
        }
    }

    fn handle_publish_confirmation(
        &mut self,
        target: CommentTarget,
        previous_focus: Pane,
        input: Input,
    ) {
        self.mode = Mode::Normal;
        self.focus = previous_focus;
        if input != Input::Character('y') {
            self.status = "Publish cancelled".to_owned();
            return;
        }
        self.confirm_publish(target);
    }

    fn handle_edit_input(&mut self, previous_focus: Pane, input: Input) {
        self.pending_g = false;
        match input {
            Input::Character(character) => self.insert_edit_character(character),
            Input::Enter => self.insert_edit_character('\n'),
            Input::Backspace => self.edit_backspace(),
            Input::Left => self.move_edit_left(),
            Input::Right => self.move_edit_right(),
            Input::Up => self.move_edit_vertical(true),
            Input::Down => self.move_edit_vertical(false),
            Input::Home => self.move_edit_home(),
            Input::End => self.move_edit_end(),
            Input::Escape => {
                self.persist_edit();
            }
            Input::Cancel => self.cancel_edit(previous_focus),
            Input::ExternalEditor => self.request_external_editor_from_edit(),
            Input::HalfPageDown | Input::HalfPageUp | Input::Unrelated | Input::Quit => {}
        }
    }

    fn apply_normal(&mut self, command: Command) {
        match command {
            Command::Quit => {
                self.quit();
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

    fn quit(&mut self) {
        self.cancel_active_detail();
        if let Some(active) = self.active_comment_load.take() {
            self.comment_effects.push_back(CommentEffect::Cancel {
                request_id: active.request_id,
            });
        }
        if let Some(active) = self.active_publish.take() {
            self.comment_effects.push_back(CommentEffect::Cancel {
                request_id: active.request_id,
            });
        }
        self.should_quit = true;
        self.status = "Closing ReviewBox".to_owned();
    }

    fn open_comment_editor(&mut self) {
        let target = match self.comment_target_for_focus() {
            Ok(target) => target,
            Err(message) => {
                self.status = message.to_owned();
                return;
            }
        };
        if self
            .drafts
            .get(&target)
            .is_some_and(|draft| draft.submission.is_some())
        {
            self.status = "Publish outcome is unverified; reconcile before editing".to_owned();
            return;
        }
        let saved = self.drafts.get(&target).map(|draft| draft.body.clone());
        let text = saved.clone().unwrap_or_default();
        let cursor = text.len();
        let previous_focus = self.focus;
        self.edit_buffer = Some(EditBuffer {
            text,
            cursor,
            saved,
        });
        self.mode = Mode::Edit {
            target,
            previous_focus,
        };
        self.status = "Editing comment draft".to_owned();
    }

    fn comment_target_for_focus(&self) -> Result<CommentTarget, &'static str> {
        match self.focus {
            Pane::Repository => Err("Select a commit before writing a comment"),
            Pane::Commit | Pane::File => self.current_commit_target(),
            Pane::Diff => self.current_line_comment_target(),
        }
    }

    fn request_external_editor_from_normal(&mut self) {
        let target = match self.comment_target_for_focus() {
            Ok(target) => target,
            Err(message) => {
                self.status = message.to_owned();
                return;
            }
        };
        if self
            .drafts
            .get(&target)
            .is_some_and(|draft| draft.submission.is_some())
        {
            self.status = "Publish outcome is unverified; reconcile before editing".to_owned();
            return;
        }
        let body = self
            .drafts
            .get(&target)
            .map(|draft| draft.body.clone())
            .unwrap_or_default();
        self.editor_requests.push_back(EditorRequest {
            target,
            body,
            origin: EditorOrigin::Normal,
        });
        self.status = "Opening external editor".to_owned();
    }

    fn request_external_editor_from_edit(&mut self) {
        let Mode::Edit { target, .. } = self.mode.clone() else {
            return;
        };
        let Some(buffer) = self.edit_buffer.as_ref() else {
            self.status = "Comment editor state is unavailable".to_owned();
            return;
        };
        if self
            .drafts
            .get(&target)
            .is_some_and(|draft| draft.submission.is_some())
        {
            self.status = "Publish outcome is unverified; reconcile before editing".to_owned();
            return;
        }
        self.editor_requests.push_back(EditorRequest {
            target,
            body: buffer.text.clone(),
            origin: EditorOrigin::Edit,
        });
        self.status = "Opening external editor".to_owned();
    }

    pub fn take_editor_requests(&mut self) -> Vec<EditorRequest> {
        self.editor_requests.drain(..).collect()
    }

    #[cfg(test)]
    pub fn set_attempt_source(&mut self, source: Box<dyn AttemptSource>) {
        self.attempt_source = source;
    }

    fn request_publish(&mut self) {
        if self.active_publish.is_some() {
            self.status = "A comment publish is already in progress".to_owned();
            return;
        }
        let target = match self.comment_target_for_focus() {
            Ok(target) => target,
            Err(message) => {
                self.status = message.to_owned();
                return;
            }
        };
        let Some(draft) = self.drafts.get(&target) else {
            self.status = "No saved comment draft to publish".to_owned();
            return;
        };
        if draft.body.chars().all(char::is_whitespace) {
            self.status = "Empty comment drafts cannot be published".to_owned();
            return;
        }
        if draft.submission.is_some() {
            self.reconcile_target(target);
            return;
        }
        self.mode = Mode::ConfirmPublish {
            target,
            previous_focus: self.focus,
        };
        self.status = "Confirm publish with y; any other key cancels".to_owned();
    }

    fn confirm_publish(&mut self, target: CommentTarget) {
        if self.active_publish.is_some() {
            self.status = "A comment publish is already in progress".to_owned();
            return;
        }
        let Some(draft) = self.drafts.get(&target).cloned() else {
            self.status = "Comment draft is no longer available".to_owned();
            return;
        };
        let Ok(attempt) = self.attempt_source.next() else {
            self.status = "Could not create a safe publish identifier; draft preserved".to_owned();
            return;
        };
        let Some(store) = self.draft_store.as_ref() else {
            self.status = "Comment drafts are unavailable; nothing was published".to_owned();
            return;
        };
        let drafts = match store.begin_submission(&target, attempt.clone()) {
            Ok(drafts) => drafts,
            Err(error) => {
                self.status = format!("Publish not started; draft unchanged: {error}");
                return;
            }
        };
        let Some((key, repository)) = self.repository_for_target(&target) else {
            let _ = store
                .clear_submission(&target)
                .map(|drafts| self.drafts = drafts);
            self.status = "Publish not started; repository is unavailable".to_owned();
            return;
        };
        self.drafts = drafts;
        let request_id = self.allocate_request_id();
        let marker = format!("<!-- reviewbox-attempt:{} -->", attempt.attempt);
        let body_with_marker = format!("{}\n\n{marker}", draft.body);
        self.active_publish = Some(ActivePublish {
            request_id,
            key: key.clone(),
            target: target.clone(),
            repository: repository.clone(),
        });
        self.comment_effects.push_back(CommentEffect::Publish {
            request_id,
            key,
            target,
            repository,
            body_with_marker,
        });
        self.status = "Publishing comment".to_owned();
    }

    fn open_comments(&mut self) {
        if self.current_commit().is_none() {
            self.status = "Select a commit before opening comments".to_owned();
            return;
        }
        let previous_focus = self.focus;
        self.mode = Mode::Comments { previous_focus };
        self.comments_scroll = 0;
        self.load_current_comments(false);
    }

    fn load_current_comments(&mut self, force: bool) {
        let Some((key, repository)) = self.current_detail_target() else {
            self.status = "No commit is selected".to_owned();
            return;
        };
        if !force
            && matches!(
                self.comment_cache.get(&key),
                Some(CommentListState::Loading)
            )
        {
            return;
        }
        if let Some(active) = self.active_comment_load.take() {
            self.comment_effects.push_back(CommentEffect::Cancel {
                request_id: active.request_id,
            });
        }
        let reconcile_target = self
            .drafts
            .iter()
            .find(|draft| {
                draft.target.repository_id() == key.repository_id
                    && draft.target.sha() == key.sha
                    && draft.submission.is_some()
            })
            .map(|draft| draft.target.clone());
        let marker = reconcile_target
            .as_ref()
            .and_then(|target| self.drafts.get(target)?.submission.as_ref())
            .map(|submission| format!("<!-- reviewbox-attempt:{} -->", submission.attempt));
        let request_id = self.allocate_request_id();
        self.comment_cache
            .insert(key.clone(), CommentListState::Loading);
        self.active_comment_load = Some(ActiveCommentLoad {
            request_id,
            key: key.clone(),
        });
        self.comment_effects.push_back(CommentEffect::Load {
            request_id,
            key,
            repository,
            marker,
            reconcile_target,
        });
        self.status = "Loading commit comments".to_owned();
    }

    fn reconcile_target(&mut self, target: CommentTarget) {
        let Some((key, repository)) = self.repository_for_target(&target) else {
            self.status = "Cannot reconcile while the repository is unavailable".to_owned();
            return;
        };
        if let Some(active) = self.active_comment_load.take() {
            self.comment_effects.push_back(CommentEffect::Cancel {
                request_id: active.request_id,
            });
        }
        let marker = self
            .drafts
            .get(&target)
            .and_then(|draft| draft.submission.as_ref())
            .map(|submission| format!("<!-- reviewbox-attempt:{} -->", submission.attempt));
        let request_id = self.allocate_request_id();
        self.comment_cache
            .insert(key.clone(), CommentListState::Loading);
        self.active_comment_load = Some(ActiveCommentLoad {
            request_id,
            key: key.clone(),
        });
        self.comment_effects.push_back(CommentEffect::Load {
            request_id,
            key,
            repository,
            marker,
            reconcile_target: Some(target),
        });
        self.status = "Reconciling the previous publish attempt".to_owned();
    }

    fn repository_for_target(
        &self,
        target: &CommentTarget,
    ) -> Option<(DetailKey, RepositoryIdentity)> {
        let repository = self
            .inbox
            .repositories
            .iter()
            .find(|repository| repository.identity.id == target.repository_id())?;
        Some((
            DetailKey {
                repository_id: target.repository_id(),
                sha: target.sha().to_owned(),
            },
            repository.identity.clone(),
        ))
    }

    fn allocate_request_id(&mut self) -> u64 {
        let id = self.next_request_id;
        self.next_request_id = self.next_request_id.saturating_add(1);
        id
    }

    pub fn take_comment_effects(&mut self) -> Vec<CommentEffect> {
        self.comment_effects.drain(..).collect()
    }

    pub fn apply_comment_result(&mut self, result: CommentResult) {
        match result.outcome {
            CommentResultOutcome::Loaded(outcome) => {
                let current = self.active_comment_load.as_ref().is_some_and(|active| {
                    active.request_id == result.request_id && active.key == result.key
                });
                if !current {
                    return;
                }
                self.active_comment_load = None;
                match outcome {
                    Ok(list) => {
                        let count = list.comments.len();
                        let complete = list.complete;
                        let found = list.marker_found;
                        self.comment_cache
                            .insert(result.key, CommentListState::Loaded(list));
                        if let Some(target) = result.target {
                            self.apply_reconciliation(target, found, complete);
                        } else {
                            self.status = if count == 0 {
                                "No existing comments".to_owned()
                            } else {
                                format!("Loaded {count} existing comments")
                            };
                        }
                    }
                    Err(error) => {
                        self.comment_cache
                            .insert(result.key, CommentListState::Failed(error));
                        self.status = "Could not load commit comments".to_owned();
                    }
                }
            }
            CommentResultOutcome::Published(outcome) => {
                let current = self.active_publish.as_ref().is_some_and(|active| {
                    active.request_id == result.request_id
                        && active.key == result.key
                        && Some(&active.target) == result.target.as_ref()
                });
                if !current {
                    return;
                }
                let active = self.active_publish.take().expect("checked active publish");
                match outcome {
                    PublishOutcome::Created { .. } => {
                        match self
                            .draft_store
                            .as_ref()
                            .ok_or(DraftStateError::Unavailable)
                            .and_then(|store| store.delete(&active.target))
                        {
                            Ok(drafts) => {
                                self.drafts = drafts;
                                self.status = "Comment published".to_owned();
                                self.queue_comment_load(active.key, active.repository, None, None);
                            }
                            Err(error) => {
                                self.status = format!(
                                    "Comment was created, but its local draft could not be cleared: {error}"
                                )
                            }
                        }
                    }
                    PublishOutcome::DefinitelyNotCreated(error) => {
                        match self
                            .draft_store
                            .as_ref()
                            .ok_or(DraftStateError::Unavailable)
                            .and_then(|store| store.clear_submission(&active.target))
                        {
                            Ok(drafts) => {
                                self.drafts = drafts;
                                self.status =
                                    format!("Comment was not published; draft preserved: {error}");
                            }
                            Err(store_error) => {
                                self.status = format!(
                                    "Publish failed and its lock could not be cleared: {store_error}"
                                )
                            }
                        }
                    }
                    PublishOutcome::Unverified(error) => {
                        self.status = format!(
                            "Publish outcome is unverified; draft locked for reconciliation: {error}"
                        )
                    }
                }
            }
        }
    }

    fn apply_reconciliation(&mut self, target: CommentTarget, found: bool, complete: bool) {
        if found {
            match self
                .draft_store
                .as_ref()
                .ok_or(DraftStateError::Unavailable)
                .and_then(|store| store.delete(&target))
            {
                Ok(drafts) => {
                    self.drafts = drafts;
                    self.status = "Previous publish confirmed; draft cleared".to_owned();
                }
                Err(error) => {
                    self.status =
                        format!("Publish confirmed, but local draft could not be cleared: {error}")
                }
            }
            return;
        }
        let old_enough = self
            .drafts
            .get(&target)
            .and_then(|draft| draft.submission.as_ref())
            .is_some_and(|submission| {
                self.attempt_source
                    .now()
                    .signed_duration_since(submission.started_at)
                    .num_seconds()
                    >= 60
            });
        if complete && old_enough {
            match self
                .draft_store
                .as_ref()
                .ok_or(DraftStateError::Unavailable)
                .and_then(|store| store.clear_submission(&target))
            {
                Ok(drafts) => {
                    self.drafts = drafts;
                    self.status =
                        "No matching comment found; publish lock cleared for a new attempt"
                            .to_owned();
                }
                Err(error) => self.status = format!("Could not clear the publish lock: {error}"),
            }
        } else if !complete {
            self.status = "Comment listing is incomplete; publish remains unverified".to_owned();
        } else {
            self.status = "No matching comment yet; wait before retrying".to_owned();
        }
    }

    fn queue_comment_load(
        &mut self,
        key: DetailKey,
        repository: RepositoryIdentity,
        marker: Option<String>,
        reconcile_target: Option<CommentTarget>,
    ) {
        if let Some(active) = self.active_comment_load.take() {
            self.comment_effects.push_back(CommentEffect::Cancel {
                request_id: active.request_id,
            });
        }
        let request_id = self.allocate_request_id();
        self.comment_cache
            .insert(key.clone(), CommentListState::Loading);
        self.active_comment_load = Some(ActiveCommentLoad {
            request_id,
            key: key.clone(),
        });
        self.comment_effects.push_back(CommentEffect::Load {
            request_id,
            key,
            repository,
            marker,
            reconcile_target,
        });
    }

    pub fn apply_editor_outcome(&mut self, request: EditorRequest, outcome: EditorOutcome) {
        match outcome {
            EditorOutcome::Replaced(body) => {
                let had_saved_draft = self.drafts.get(&request.target).is_some();
                let result =
                    CommentDraft::new(request.target.clone(), body.clone()).and_then(|draft| {
                        self.draft_store
                            .as_ref()
                            .ok_or(DraftStateError::Unavailable)?
                            .save(&draft)
                    });
                match result {
                    Ok(drafts) => {
                        self.drafts = drafts;
                        if request.origin == EditorOrigin::Edit {
                            self.replace_matching_edit_buffer(&request.target, body, true);
                        }
                        self.status = if had_saved_draft {
                            "Comment draft saved from external editor"
                        } else {
                            "New comment draft saved from external editor"
                        }
                        .to_owned();
                    }
                    Err(error) => {
                        if request.origin == EditorOrigin::Edit {
                            self.replace_matching_edit_buffer(&request.target, body, false);
                        }
                        self.status = format!("External editor text not saved: {error}");
                    }
                }
            }
            EditorOutcome::Unchanged => {
                self.status = "External editor left the draft unchanged".to_owned();
            }
            EditorOutcome::Failed(error) => {
                self.status = format!("External editor failed: {error}");
            }
        }
    }

    fn replace_matching_edit_buffer(&mut self, target: &CommentTarget, body: String, saved: bool) {
        if !matches!(&self.mode, Mode::Edit { target: active, .. } if active == target) {
            return;
        }
        if let Some(buffer) = self.edit_buffer.as_mut() {
            buffer.text = body;
            buffer.cursor = buffer.text.len();
            if saved {
                buffer.saved = Some(buffer.text.clone());
            }
        }
    }

    fn current_commit_target(&self) -> Result<CommentTarget, &'static str> {
        let Some(repository) = self.current_repository() else {
            return Err("No repository is selected");
        };
        let Some(commit) = self.current_commit() else {
            return Err("No commit is selected");
        };
        CommentTarget::commit(repository.identity.id, &commit.sha)
            .map_err(|_| "Selected commit cannot accept a draft")
    }

    fn current_line_comment_target(&self) -> Result<CommentTarget, &'static str> {
        let Some(repository) = self.current_repository() else {
            return Err("No repository is selected");
        };
        let Some(commit) = self.current_commit() else {
            return Err("No commit is selected");
        };
        let Some(file) = self.current_file() else {
            return Err("No diff file is selected");
        };
        let Some(line) = line_target(file, self.diff_cursor) else {
            return Err("Selected diff row cannot accept a line comment");
        };
        CommentTarget::line(repository.identity.id, &commit.sha, line)
            .map_err(|_| "Selected diff row cannot accept a line comment")
    }

    fn insert_edit_character(&mut self, character: char) {
        if character != '\n' && character.is_control() {
            self.status = "Only printable text can be inserted".to_owned();
            return;
        }
        let Some(buffer) = self.edit_buffer.as_mut() else {
            return;
        };
        if buffer.text.chars().count() >= MAX_DRAFT_CHARACTERS {
            self.status = format!("Draft limit reached ({MAX_DRAFT_CHARACTERS} characters)");
            return;
        }
        buffer.text.insert(buffer.cursor, character);
        buffer.cursor += character.len_utf8();
    }

    fn edit_backspace(&mut self) {
        let Some(buffer) = self.edit_buffer.as_mut() else {
            return;
        };
        if buffer.cursor == 0 {
            return;
        }
        let previous = previous_char_boundary(&buffer.text, buffer.cursor);
        buffer.text.drain(previous..buffer.cursor);
        buffer.cursor = previous;
    }

    fn move_edit_left(&mut self) {
        if let Some(buffer) = self.edit_buffer.as_mut() {
            buffer.cursor = previous_char_boundary(&buffer.text, buffer.cursor);
        }
    }

    fn move_edit_right(&mut self) {
        if let Some(buffer) = self.edit_buffer.as_mut() {
            buffer.cursor = next_char_boundary(&buffer.text, buffer.cursor);
        }
    }

    fn move_edit_vertical(&mut self, upward: bool) {
        let Some(buffer) = self.edit_buffer.as_mut() else {
            return;
        };
        buffer.cursor = vertical_cursor(&buffer.text, buffer.cursor, upward);
    }

    fn move_edit_home(&mut self) {
        if let Some(buffer) = self.edit_buffer.as_mut() {
            buffer.cursor = current_line_bounds(&buffer.text, buffer.cursor).0;
        }
    }

    fn move_edit_end(&mut self) {
        if let Some(buffer) = self.edit_buffer.as_mut() {
            buffer.cursor = current_line_bounds(&buffer.text, buffer.cursor).1;
        }
    }

    fn cancel_edit(&mut self, previous_focus: Pane) {
        if let Some(buffer) = self.edit_buffer.as_mut() {
            buffer.text = buffer.saved.clone().unwrap_or_default();
            buffer.cursor = buffer.text.len();
        }
        self.edit_buffer = None;
        self.focus = previous_focus;
        self.mode = Mode::Normal;
        self.status = "Comment edit cancelled; saved draft unchanged".to_owned();
    }

    fn persist_edit(&mut self) -> bool {
        let Mode::Edit {
            target,
            previous_focus,
        } = self.mode.clone()
        else {
            return true;
        };
        let Some(buffer) = self.edit_buffer.as_ref() else {
            self.status = "Comment editor state is unavailable".to_owned();
            return false;
        };
        let body = buffer.text.clone();
        let had_saved_draft = buffer.saved.is_some();
        let deleting_saved_draft = had_saved_draft && body.chars().all(char::is_whitespace);

        let result = if body.chars().all(char::is_whitespace) {
            if had_saved_draft {
                self.draft_store
                    .as_ref()
                    .ok_or(DraftStateError::Unavailable)
                    .and_then(|store| store.delete(&target))
                    .map(Some)
            } else {
                Ok(None)
            }
        } else {
            CommentDraft::new(target.clone(), body)
                .and_then(|draft| {
                    self.draft_store
                        .as_ref()
                        .ok_or(DraftStateError::Unavailable)?
                        .save(&draft)
                })
                .map(Some)
        };

        match result {
            Ok(Some(drafts)) => {
                self.drafts = drafts;
                self.finish_edit(
                    previous_focus,
                    if deleting_saved_draft {
                        "Blank comment draft deleted"
                    } else if had_saved_draft {
                        "Comment draft saved"
                    } else {
                        "New comment draft saved"
                    },
                );
                true
            }
            Ok(None) => {
                self.finish_edit(
                    previous_focus,
                    if had_saved_draft {
                        "Blank comment draft deleted"
                    } else {
                        "Blank new comment discarded"
                    },
                );
                true
            }
            Err(error) => {
                self.status = format!("Comment draft not saved: {error}");
                false
            }
        }
    }

    fn finish_edit(&mut self, previous_focus: Pane, status: &'static str) {
        self.edit_buffer = None;
        self.focus = previous_focus;
        self.mode = Mode::Normal;
        self.status = status.to_owned();
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
        if before != self.current_detail_key() {
            self.reset_diff_position();
            if self.focus > Pane::Commit {
                self.focus = Pane::Commit;
            }
        } else {
            self.clear_diff_match();
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
        self.reset_diff_position();
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
            Pane::Diff => self.diff_cursor,
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
                    self.diff_cursor = index;
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
                self.diff_cursor = moved_index(
                    self.diff_cursor,
                    self.current_diff_lines().len(),
                    upward,
                    amount,
                );
                self.ensure_diff_line_visible(self.diff_cursor);
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
                self.diff_cursor = 0;
                self.ensure_diff_line_visible(0);
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
                self.diff_cursor = self.current_diff_lines().len().saturating_sub(1);
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
            self.reset_diff_position();
            self.cancel_detail_if_selection_changed();
        }
    }

    fn select_commit(&mut self, selected: usize) {
        if selected != self.commits.selected {
            self.commits.selected = selected;
            self.files = ListPosition::default();
            self.reset_diff_position();
            self.cancel_detail_if_selection_changed();
        }
    }

    fn select_file(&mut self, selected: usize) {
        if selected != self.files.selected {
            self.files.selected = selected;
            self.reset_diff_position();
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

        let diff_len = self.current_diff_lines().len();
        self.diff_cursor = self.diff_cursor.min(diff_len.saturating_sub(1));
        self.diff_scroll = self.diff_scroll.min(self.max_diff_scroll());
        self.diff_row_offset = self
            .diff_row_offset
            .min(self.diff_line_rows(self.diff_scroll).saturating_sub(1));
        let tail_requested = diff_len > 0
            && self.diff_cursor == diff_len.saturating_sub(1)
            && self.diff_row_offset > 0;
        if tail_requested {
            self.diff_scroll = self.max_diff_scroll();
            self.diff_row_offset = self.tail_diff_row_offset();
        } else if diff_len > 0 {
            self.ensure_diff_line_visible(self.diff_cursor);
        }
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

    fn reset_diff_position(&mut self) {
        self.diff_cursor = 0;
        self.diff_scroll = 0;
        self.diff_row_offset = 0;
        self.diff_match = None;
    }

    pub fn diff_match(&self) -> Option<usize> {
        self.diff_match
    }

    pub fn diff_cursor(&self) -> usize {
        self.diff_cursor
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
            .saturating_add(5)
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
        self.mode.clone()
    }

    pub fn focus(&self) -> Pane {
        self.focus
    }

    pub fn selected(&self, pane: Pane) -> usize {
        match pane {
            Pane::Repository => self.repositories.selected,
            Pane::Commit => self.commits.selected,
            Pane::File => self.files.selected,
            Pane::Diff => self.diff_cursor,
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

    pub fn current_comments(&self) -> Option<&CommentListState> {
        self.comment_cache.get(&self.current_detail_key()?)
    }

    pub fn comments_scroll(&self) -> usize {
        self.comments_scroll
    }

    pub fn publish_in_flight(&self) -> bool {
        self.active_publish.is_some()
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

    pub fn edit_buffer(&self) -> Option<&EditBuffer> {
        self.edit_buffer.as_ref()
    }

    pub fn commit_has_draft(&self, repository_id: u64, sha: &str) -> bool {
        CommentTarget::commit(repository_id, sha)
            .ok()
            .is_some_and(|target| self.drafts.get(&target).is_some())
    }

    pub fn commit_has_comments(&self, repository_id: u64, sha: &str) -> bool {
        self.comment_cache.get(&DetailKey { repository_id, sha: sha.to_owned() }).is_some_and(|state| matches!(state, CommentListState::Loaded(list) if !list.comments.is_empty()))
    }

    pub fn diff_line_has_draft(&self, row: usize) -> bool {
        let Some(repository) = self.current_repository() else {
            return false;
        };
        let Some(commit) = self.current_commit() else {
            return false;
        };
        let Some(file) = self.current_file() else {
            return false;
        };
        line_target(file, row)
            .and_then(|line| CommentTarget::line(repository.identity.id, &commit.sha, line).ok())
            .is_some_and(|target| self.drafts.get(&target).is_some())
    }

    pub fn diff_line_has_comment(&self, row: usize) -> bool {
        let Some(file) = self.current_file() else {
            return false;
        };
        let Some(target) = line_target(file, row) else {
            return false;
        };
        matches!(self.current_comments(), Some(CommentListState::Loaded(list)) if list.comments.iter().any(|comment| matches!(&comment.anchor, ExistingCommentAnchor::Line { path, position: Some(position), .. } if path == &target.path && *position == target.position)))
    }

    pub fn diff_comment_feedback(&self) -> String {
        let Some(file) = self.current_file() else {
            return "line comments unavailable: no diff".to_owned();
        };
        if let Some(target) = line_target(file, self.diff_cursor) {
            return format!("commentable {}:{}", target.path, target.position);
        }
        if !file.api_path_is_commentable {
            return "line comments unavailable: unsafe API path".to_owned();
        }
        let lines = match &file.patch {
            PatchContent::Text { lines }
            | PatchContent::Capped {
                lines,
                reason: PatchCapReason::FileLimit,
                ..
            } => lines,
            PatchContent::Capped {
                reason: PatchCapReason::CommitBudget,
                ..
            } => return "line comments unavailable: commit-budget omission".to_owned(),
            PatchContent::Empty => return "line comments unavailable: empty patch".to_owned(),
            PatchContent::NoPatch | PatchContent::Unavailable => {
                return "line comments unavailable: no text patch".to_owned();
            }
        };
        if lines.first().map(|line| line.kind) != Some(DiffLineKind::Hunk) {
            return "line comments unavailable: invalid hunk".to_owned();
        }
        match lines.get(self.diff_cursor).map(|line| line.kind) {
            Some(DiffLineKind::Hunk) => "line comments unavailable: hunk header".to_owned(),
            Some(DiffLineKind::NoNewline) => {
                "line comments unavailable: no-newline marker".to_owned()
            }
            Some(DiffLineKind::Other) => {
                "line comments unavailable: unsupported patch row".to_owned()
            }
            Some(DiffLineKind::Context | DiffLineKind::Addition | DiffLineKind::Deletion) => {
                "line comments unavailable: invalid target".to_owned()
            }
            None => "line comments unavailable: no selected row".to_owned(),
        }
    }

    pub fn edit_target_description(&self, target: &CommentTarget) -> String {
        let short_sha: String = target.sha().chars().take(7).collect();
        match target.anchor() {
            CommentAnchor::Commit => format!("commit {short_sha}"),
            CommentAnchor::Line(line) => {
                let line_number = self
                    .target_line_numbers(target)
                    .map(|(old, new)| match (old, new) {
                        (Some(old), Some(new)) => format!("old {old}, new {new}"),
                        (Some(old), None) => format!("old {old}"),
                        (None, Some(new)) => format!("new {new}"),
                        (None, None) => "line number unavailable".to_owned(),
                    })
                    .unwrap_or_else(|| "line number unavailable".to_owned());
                format!(
                    "{short_sha} • {}:{} • {line_number}",
                    line.path, line.position
                )
            }
        }
    }

    fn target_line_numbers(&self, target: &CommentTarget) -> Option<(Option<u32>, Option<u32>)> {
        let CommentAnchor::Line(line) = target.anchor() else {
            return None;
        };
        let row = usize::try_from(line.position).ok()?;
        let repository = self
            .inbox
            .repositories
            .iter()
            .find(|repository| repository.identity.id == target.repository_id())?;
        let commit = repository
            .commits
            .iter()
            .find(|commit| commit.sha == target.sha())?;
        let files = if self.inbox.child_panes_available() {
            commit.files.as_slice()
        } else {
            let key = DetailKey {
                repository_id: target.repository_id(),
                sha: target.sha().to_owned(),
            };
            match self.detail_cache.get(&key) {
                Some(DetailState::Ready(detail)) => &detail.files,
                _ => return None,
            }
        };
        let content = files.iter().find(|file| file.path == line.path)?;
        let diff_line = content.patch.lines().get(row)?;
        Some((diff_line.old_line, diff_line.new_line))
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
        if self
            .active_comment_load
            .as_ref()
            .is_some_and(|active| Some(&active.key) != selected.as_ref())
            && let Some(active) = self.active_comment_load.take()
        {
            self.comment_cache.remove(&active.key);
            self.comment_effects.push_back(CommentEffect::Cancel {
                request_id: active.request_id,
            });
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

fn previous_char_boundary(text: &str, cursor: usize) -> usize {
    text[..cursor]
        .char_indices()
        .next_back()
        .map_or(0, |(index, _)| index)
}

fn next_char_boundary(text: &str, cursor: usize) -> usize {
    text[cursor..]
        .chars()
        .next()
        .map_or(text.len(), |character| cursor + character.len_utf8())
}

fn current_line_bounds(text: &str, cursor: usize) -> (usize, usize) {
    let start = text[..cursor].rfind('\n').map_or(0, |index| index + 1);
    let end = text[cursor..]
        .find('\n')
        .map_or(text.len(), |index| cursor + index);
    (start, end)
}

fn vertical_cursor(text: &str, cursor: usize, upward: bool) -> usize {
    let (start, end) = current_line_bounds(text, cursor);
    let column = text[start..cursor].chars().count();
    let (target_start, target_end) = if upward {
        if start == 0 {
            return cursor;
        }
        let target_end = start - 1;
        let target_start = text[..target_end].rfind('\n').map_or(0, |index| index + 1);
        (target_start, target_end)
    } else {
        if end == text.len() {
            return cursor;
        }
        let target_start = end + 1;
        let target_end = text[target_start..]
            .find('\n')
            .map_or(text.len(), |index| target_start + index);
        (target_start, target_end)
    };
    text[target_start..target_end]
        .char_indices()
        .nth(column)
        .map_or(target_end, |(index, _)| target_start + index)
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

    fn type_edit_text(app: &mut App, text: &str) {
        for character in text.chars() {
            app.handle_input(if character == '\n' {
                Input::Enter
            } else {
                Input::Character(character)
            });
        }
    }

    fn open_first_commit_editor(app: &mut App) {
        app.handle_input(Input::Character('l'));
        assert_eq!(app.focus(), Pane::Commit);
        app.handle_input(Input::Character('c'));
        assert!(matches!(app.mode(), Mode::Edit { .. }));
    }

    #[test]
    fn keyboard_commit_draft_supports_multiline_cursor_edit_save_and_reopen() {
        let mut app = App::new(DemoFixture::load());
        open_first_commit_editor(&mut app);
        type_edit_text(&mut app, "alpha\nbeta");
        app.handle_input(Input::Escape);

        assert_eq!(app.mode(), Mode::Normal);
        assert_eq!(app.focus(), Pane::Commit);
        assert_eq!(app.draft_count(), 1);
        app.handle_input(Input::Character('c'));
        assert_eq!(app.edit_buffer().unwrap().text(), "alpha\nbeta");
        assert!(app.edit_buffer().unwrap().is_saved());

        app.handle_input(Input::Up);
        app.handle_input(Input::Home);
        app.handle_input(Input::Right);
        app.handle_input(Input::Character('X'));
        app.handle_input(Input::Down);
        app.handle_input(Input::End);
        app.handle_input(Input::Character('!'));
        app.handle_input(Input::Escape);
        app.handle_input(Input::Character('c'));
        assert_eq!(app.edit_buffer().unwrap().text(), "aXlpha\nbeta!");
    }

    #[test]
    fn every_normal_mode_character_is_inserted_while_editing() {
        let mut app = App::new(DemoFixture::load());
        open_first_commit_editor(&mut app);
        let focus = app.focus();
        let characters = "hjklqnN/?mfgGcEPC";
        type_edit_text(&mut app, characters);

        assert_eq!(app.edit_buffer().unwrap().text(), characters);
        assert_eq!(app.focus(), focus);
        assert!(!app.should_quit());
        assert!(matches!(app.mode(), Mode::Edit { .. }));
    }

    #[test]
    fn normal_e_queues_the_context_draft_without_entering_edit_mode() {
        let mut app = App::new(DemoFixture::load());
        app.handle_input(Input::Character('E'));
        assert!(app.take_editor_requests().is_empty());
        assert!(app.status().contains("Select a commit"));

        app.handle_input(Input::Character('l'));
        app.handle_input(Input::Character('E'));
        let requests = app.take_editor_requests();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].origin, EditorOrigin::Normal);
        assert!(requests[0].body.is_empty());
        assert!(matches!(requests[0].target.anchor(), CommentAnchor::Commit));
        assert_eq!(app.mode(), Mode::Normal);
    }

    #[test]
    fn ctrl_e_replaces_and_saves_the_current_edit_buffer_without_leaving_edit() {
        let mut app = App::new(DemoFixture::load());
        open_first_commit_editor(&mut app);
        type_edit_text(&mut app, "before");
        app.handle_input(Input::ExternalEditor);
        let request = app.take_editor_requests().pop().unwrap();
        assert_eq!(request.origin, EditorOrigin::Edit);
        assert_eq!(request.body, "before");

        app.apply_editor_outcome(
            request,
            EditorOutcome::Replaced("after\nsecond line".to_owned()),
        );

        assert!(matches!(app.mode(), Mode::Edit { .. }));
        assert_eq!(app.edit_buffer().unwrap().text(), "after\nsecond line");
        assert!(app.edit_buffer().unwrap().is_saved());
        assert_eq!(app.draft_count(), 1);
    }

    #[test]
    fn unsuccessful_external_exit_preserves_the_saved_draft() {
        let mut app = App::new(DemoFixture::load());
        open_first_commit_editor(&mut app);
        type_edit_text(&mut app, "durable body");
        app.handle_input(Input::Escape);
        app.handle_input(Input::Character('E'));
        let request = app.take_editor_requests().pop().unwrap();
        let target = request.target.clone();

        app.apply_editor_outcome(request, EditorOutcome::Unchanged);

        assert_eq!(app.drafts.get(&target).unwrap().body, "durable body");
        assert_eq!(app.status(), "External editor left the draft unchanged");
    }

    #[test]
    fn external_editor_action_is_blocked_by_search_and_help_modals() {
        let mut app = App::new(DemoFixture::load());
        app.handle_input(Input::Character('/'));
        app.handle_input(Input::ExternalEditor);
        assert!(app.take_editor_requests().is_empty());
        assert!(matches!(app.mode(), Mode::SearchEntry { .. }));
        app.handle_input(Input::Escape);

        app.handle_input(Input::Character('?'));
        app.handle_input(Input::ExternalEditor);
        assert!(app.take_editor_requests().is_empty());
        assert!(matches!(app.mode(), Mode::Help { .. }));
    }

    #[test]
    fn keyboard_line_drafts_reject_unsupported_rows_and_keep_targets_separate() {
        let mut app = App::new(DemoFixture::load());
        for key in ['l', 'l', 'l'] {
            app.handle_input(Input::Character(key));
        }
        assert_eq!(app.focus(), Pane::Diff);
        app.handle_input(Input::Character('c'));
        assert_eq!(app.mode(), Mode::Normal);
        assert_eq!(app.draft_count(), 0);
        assert!(app.status().contains("cannot accept"));

        app.handle_input(Input::Character('j'));
        app.handle_input(Input::Character('c'));
        let first_target = match app.mode() {
            Mode::Edit { target, .. } => target,
            mode => panic!("expected line editor, got {mode:?}"),
        };
        assert!(matches!(first_target.anchor(), CommentAnchor::Line(_)));
        type_edit_text(&mut app, "first line draft");
        app.handle_input(Input::Escape);

        app.handle_input(Input::Character('h'));
        app.handle_input(Input::Character('j'));
        app.handle_input(Input::Character('l'));
        app.handle_input(Input::Character('j'));
        app.handle_input(Input::Character('c'));
        let second_target = match app.mode() {
            Mode::Edit { target, .. } => target,
            mode => panic!("expected second line editor, got {mode:?}"),
        };
        assert_ne!(first_target, second_target);
        type_edit_text(&mut app, "second line draft");
        app.handle_input(Input::Escape);

        assert_eq!(app.draft_count(), 2);
        assert_eq!(
            app.drafts.get(&first_target).unwrap().body,
            "first line draft"
        );
        assert_eq!(
            app.drafts.get(&second_target).unwrap().body,
            "second line draft"
        );
    }

    #[test]
    fn cancel_restores_the_last_saved_body_and_blank_save_deletes_it() {
        let mut app = App::new(DemoFixture::load());
        open_first_commit_editor(&mut app);
        type_edit_text(&mut app, "saved");
        app.handle_input(Input::Escape);
        app.handle_input(Input::Character('c'));
        type_edit_text(&mut app, " but cancelled");
        app.handle_input(Input::Cancel);
        assert_eq!(app.mode(), Mode::Normal);

        app.handle_input(Input::Character('c'));
        assert_eq!(app.edit_buffer().unwrap().text(), "saved");
        for _ in 0.."saved".len() {
            app.handle_input(Input::Backspace);
        }
        app.handle_input(Input::Escape);
        assert_eq!(app.draft_count(), 0);
        assert_eq!(app.status(), "Blank comment draft deleted");
    }

    #[derive(Debug)]
    struct FailingDraftStore;

    impl DraftStore for FailingDraftStore {
        fn load(&self) -> Result<CommentDrafts, DraftStateError> {
            Ok(CommentDrafts::default())
        }

        fn save(&self, _draft: &CommentDraft) -> Result<CommentDrafts, DraftStateError> {
            Err(DraftStateError::Write(std::io::ErrorKind::PermissionDenied))
        }

        fn delete(&self, _target: &CommentTarget) -> Result<CommentDrafts, DraftStateError> {
            Err(DraftStateError::Write(std::io::ErrorKind::PermissionDenied))
        }
    }

    #[derive(Debug)]
    struct SeededFailingDraftStore(CommentDrafts);

    impl DraftStore for SeededFailingDraftStore {
        fn load(&self) -> Result<CommentDrafts, DraftStateError> {
            Ok(self.0.clone())
        }

        fn save(&self, _draft: &CommentDraft) -> Result<CommentDrafts, DraftStateError> {
            Err(DraftStateError::Write(std::io::ErrorKind::PermissionDenied))
        }

        fn delete(&self, _target: &CommentTarget) -> Result<CommentDrafts, DraftStateError> {
            Err(DraftStateError::Write(std::io::ErrorKind::PermissionDenied))
        }
    }

    #[test]
    fn failed_save_and_ctrl_c_keep_editable_text_without_quitting() {
        let mut app = App::with_stores(
            DemoFixture::load(),
            Box::new(MemoryReviewStore::default()),
            Box::new(FailingDraftStore),
        );
        open_first_commit_editor(&mut app);
        type_edit_text(&mut app, "private fictional body");

        app.handle_input(Input::Escape);
        assert!(matches!(app.mode(), Mode::Edit { .. }));
        assert_eq!(app.edit_buffer().unwrap().text(), "private fictional body");
        assert!(!app.status().contains("private fictional body"));
        app.handle_input(Input::Quit);
        assert!(matches!(app.mode(), Mode::Edit { .. }));
        assert_eq!(app.edit_buffer().unwrap().text(), "private fictional body");
        assert!(!app.should_quit());
    }

    #[test]
    fn failed_external_save_preserves_prior_draft_and_keeps_new_edit_text() {
        let memory = MemoryDraftStore::default();
        let target =
            CommentTarget::commit(9_000_001, "a1b2c3d000000000000000000000000000000000").unwrap();
        memory
            .save(&CommentDraft::new(target.clone(), "saved before").unwrap())
            .unwrap();
        let drafts = memory.load().unwrap();
        let mut app = App::with_stores(
            DemoFixture::load(),
            Box::new(MemoryReviewStore::default()),
            Box::new(SeededFailingDraftStore(drafts)),
        );
        open_first_commit_editor(&mut app);
        app.handle_input(Input::ExternalEditor);
        let request = app.take_editor_requests().pop().unwrap();

        app.apply_editor_outcome(
            request,
            EditorOutcome::Replaced("recovered from editor".to_owned()),
        );

        assert_eq!(app.drafts.get(&target).unwrap().body, "saved before");
        assert_eq!(app.edit_buffer().unwrap().text(), "recovered from editor");
        assert!(app.edit_buffer().unwrap().is_saved());
        assert!(!app.status().contains("recovered from editor"));
    }

    #[test]
    fn successful_ctrl_c_saves_before_quitting() {
        let mut app = App::new(DemoFixture::load());
        open_first_commit_editor(&mut app);
        type_edit_text(&mut app, "save before quit");

        app.handle_input(Input::Quit);

        assert!(app.should_quit());
        assert_eq!(app.mode(), Mode::Normal);
        assert_eq!(app.draft_count(), 1);
    }

    #[test]
    fn editor_refuses_text_beyond_the_character_limit() {
        let target =
            CommentTarget::commit(9_000_001, "a1b2c3d000000000000000000000000000000000").unwrap();
        let draft_store = MemoryDraftStore::default();
        draft_store
            .save(&CommentDraft::new(target, "x".repeat(MAX_DRAFT_CHARACTERS)).unwrap())
            .unwrap();
        let mut app = App::with_stores(
            DemoFixture::load(),
            Box::new(MemoryReviewStore::default()),
            Box::new(draft_store),
        );
        open_first_commit_editor(&mut app);

        app.handle_input(Input::Character('y'));

        assert_eq!(
            app.edit_buffer().unwrap().text().chars().count(),
            MAX_DRAFT_CHARACTERS
        );
        assert!(app.status().contains("Draft limit reached"));
    }

    #[test]
    fn edit_target_survives_live_snapshot_reordering_and_disappearance() {
        let mut app = live_app();
        let repository = loaded_repository("fixture/edit-stability", commits());
        app.apply_load_event(LoadEvent::RepositorySnapshot {
            repository_index: 0,
            repository: repository.clone(),
        });
        open_first_commit_editor(&mut app);
        let original_target = match app.mode() {
            Mode::Edit { target, .. } => target,
            _ => unreachable!(),
        };

        let mut reordered = repository;
        reordered.repository.commits.reverse();
        app.apply_load_event(LoadEvent::RepositorySnapshot {
            repository_index: 0,
            repository: reordered,
        });
        app.apply_load_event(LoadEvent::RepositorySnapshot {
            repository_index: 0,
            repository: loaded_repository("fixture/edit-stability", Vec::new()),
        });

        assert!(matches!(
            app.mode(),
            Mode::Edit { ref target, .. } if target == &original_target
        ));
        type_edit_text(&mut app, "stable identity");
        app.handle_input(Input::Escape);
        assert_eq!(
            app.drafts.get(&original_target).unwrap().body,
            "stable identity"
        );
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

    #[derive(Debug)]
    struct FixedAttemptSource {
        now: chrono::DateTime<Utc>,
        nonce: &'static str,
    }

    impl AttemptSource for FixedAttemptSource {
        fn next(&self) -> Result<SubmissionAttempt, ()> {
            SubmissionAttempt::new(self.nonce, self.now).map_err(|_| ())
        }
        fn now(&self) -> chrono::DateTime<Utc> {
            self.now
        }
    }

    fn app_with_commit_draft() -> App {
        let mut app = App::new(DemoFixture::load());
        app.set_attempt_source(Box::new(FixedAttemptSource {
            now: Utc.with_ymd_and_hms(2024, 1, 15, 12, 2, 0).unwrap(),
            nonce: "0123456789abcdef0123456789abcdef",
        }));
        open_first_commit_editor(&mut app);
        type_edit_text(&mut app, "ready to publish");
        app.handle_input(Input::Escape);
        app
    }

    fn begin_publish(app: &mut App) -> (u64, DetailKey, CommentTarget) {
        app.handle_input(Input::Character('P'));
        assert!(matches!(app.mode(), Mode::ConfirmPublish { .. }));
        app.handle_input(Input::Character('y'));
        let effects = app.take_comment_effects();
        assert_eq!(effects.len(), 1);
        let CommentEffect::Publish {
            request_id,
            key,
            target,
            body_with_marker,
            ..
        } = &effects[0]
        else {
            panic!("publish effect")
        };
        assert!(
            body_with_marker
                .ends_with("<!-- reviewbox-attempt:0123456789abcdef0123456789abcdef -->")
        );
        (*request_id, key.clone(), target.clone())
    }

    #[test]
    fn publish_requires_confirmation_allows_one_in_flight_and_clears_only_on_created() {
        let mut app = app_with_commit_draft();
        let (request_id, key, target) = begin_publish(&mut app);
        assert!(app.publish_in_flight());
        app.handle_input(Input::Character('P'));
        assert!(app.status().contains("already in progress"));
        app.handle_input(Input::Character('y'));
        assert!(app.take_comment_effects().is_empty());
        assert_eq!(app.draft_count(), 1);
        app.apply_comment_result(CommentResult {
            request_id: request_id + 1,
            key: key.clone(),
            target: Some(target.clone()),
            outcome: CommentResultOutcome::Published(PublishOutcome::Created { id: 8 }),
        });
        assert!(app.publish_in_flight());
        assert_eq!(app.draft_count(), 1);
        app.apply_comment_result(CommentResult {
            request_id,
            key,
            target: Some(target),
            outcome: CommentResultOutcome::Published(PublishOutcome::Created { id: 9 }),
        });
        assert_eq!(app.draft_count(), 0);
        assert!(app.status().contains("published"));
        assert!(matches!(
            app.take_comment_effects().as_slice(),
            [CommentEffect::Load { .. }]
        ));
    }

    #[test]
    fn rejected_and_unverified_publish_outcomes_preserve_the_draft() {
        let mut rejected = app_with_commit_draft();
        let (request_id, key, target) = begin_publish(&mut rejected);
        rejected.apply_comment_result(CommentResult {
            request_id,
            key,
            target: Some(target.clone()),
            outcome: CommentResultOutcome::Published(PublishOutcome::DefinitelyNotCreated(
                CommentFailure {
                    kind: crate::github::CommentFailureKind::PermissionOrNotFound,
                    http_status: Some(403),
                },
            )),
        });
        assert!(rejected.drafts.get(&target).unwrap().submission.is_none());
        assert!(rejected.status().contains("draft preserved"));

        let mut ambiguous = app_with_commit_draft();
        let (request_id, key, target) = begin_publish(&mut ambiguous);
        ambiguous.apply_comment_result(CommentResult {
            request_id,
            key,
            target: Some(target.clone()),
            outcome: CommentResultOutcome::Published(PublishOutcome::Unverified(CommentFailure {
                kind: crate::github::CommentFailureKind::Transport,
                http_status: None,
            })),
        });
        assert!(ambiguous.drafts.get(&target).unwrap().submission.is_some());
        ambiguous.handle_input(Input::Character('c'));
        assert_eq!(ambiguous.mode(), Mode::Normal);
        ambiguous.handle_input(Input::Character('E'));
        assert!(ambiguous.take_editor_requests().is_empty());
    }

    #[derive(Debug)]
    struct SubmissionFailingStore(CommentDrafts);

    impl DraftStore for SubmissionFailingStore {
        fn load(&self) -> Result<CommentDrafts, DraftStateError> {
            Ok(self.0.clone())
        }
        fn save(&self, _draft: &CommentDraft) -> Result<CommentDrafts, DraftStateError> {
            Err(DraftStateError::Unavailable)
        }
        fn delete(&self, _target: &CommentTarget) -> Result<CommentDrafts, DraftStateError> {
            Err(DraftStateError::Unavailable)
        }
    }

    #[test]
    fn cancelled_confirmation_and_failed_attempt_persistence_never_emit_a_post() {
        let mut cancelled = app_with_commit_draft();
        cancelled.handle_input(Input::Character('P'));
        cancelled.handle_input(Input::Character('n'));
        assert_eq!(cancelled.mode(), Mode::Normal);
        assert!(cancelled.take_comment_effects().is_empty());
        assert_eq!(cancelled.draft_count(), 1);

        let inbox = DemoFixture::load();
        let target = CommentTarget::commit(
            inbox.repositories[0].identity.id,
            &inbox.repositories[0].commits[0].sha,
        )
        .unwrap();
        let seed = MemoryDraftStore::default()
            .save(&CommentDraft::new(target, "safe body").unwrap())
            .unwrap();
        let mut failed = App::with_stores(
            inbox,
            Box::new(MemoryReviewStore::default()),
            Box::new(SubmissionFailingStore(seed)),
        );
        failed.handle_input(Input::Character('l'));
        failed.handle_input(Input::Character('P'));
        failed.handle_input(Input::Character('y'));
        assert!(failed.take_comment_effects().is_empty());
        assert_eq!(failed.draft_count(), 1);
        assert!(failed.status().contains("not started"));
    }

    #[test]
    fn ambiguous_attempt_reconciles_by_exact_marker_and_ignores_stale_results() {
        let mut app = app_with_commit_draft();
        let (publish_id, key, target) = begin_publish(&mut app);
        app.apply_comment_result(CommentResult {
            request_id: publish_id,
            key: key.clone(),
            target: Some(target.clone()),
            outcome: CommentResultOutcome::Published(PublishOutcome::Unverified(CommentFailure {
                kind: crate::github::CommentFailureKind::Transport,
                http_status: None,
            })),
        });
        app.handle_input(Input::Character('P'));
        let effects = app.take_comment_effects();
        let CommentEffect::Load {
            request_id,
            marker: Some(marker),
            ..
        } = &effects[0]
        else {
            panic!("reconciliation load")
        };
        let request_id = *request_id;
        app.apply_comment_result(CommentResult {
            request_id: request_id + 99,
            key: key.clone(),
            target: Some(target.clone()),
            outcome: CommentResultOutcome::Loaded(Ok(ExistingComments {
                comments: Vec::new(),
                complete: true,
                marker_found: true,
            })),
        });
        assert_eq!(app.draft_count(), 1);
        assert!(marker.contains("0123456789abcdef0123456789abcdef"));
        app.apply_comment_result(CommentResult {
            request_id,
            key,
            target: Some(target),
            outcome: CommentResultOutcome::Loaded(Ok(ExistingComments {
                comments: Vec::new(),
                complete: true,
                marker_found: true,
            })),
        });
        assert_eq!(app.draft_count(), 0);
        assert!(app.status().contains("confirmed"));
    }

    #[test]
    fn complete_old_not_found_unlocks_but_incomplete_and_young_attempts_do_not() {
        for (complete, old, unlocked) in [
            (false, true, false),
            (true, false, false),
            (true, true, true),
        ] {
            let mut app = app_with_commit_draft();
            let (publish_id, key, target) = begin_publish(&mut app);
            app.apply_comment_result(CommentResult {
                request_id: publish_id,
                key: key.clone(),
                target: Some(target.clone()),
                outcome: CommentResultOutcome::Published(PublishOutcome::Unverified(
                    CommentFailure {
                        kind: crate::github::CommentFailureKind::Transport,
                        http_status: None,
                    },
                )),
            });
            if !old {
                app.set_attempt_source(Box::new(FixedAttemptSource {
                    now: Utc.with_ymd_and_hms(2024, 1, 15, 12, 2, 30).unwrap(),
                    nonce: "fedcba9876543210fedcba9876543210",
                }));
            } else {
                app.set_attempt_source(Box::new(FixedAttemptSource {
                    now: Utc.with_ymd_and_hms(2024, 1, 15, 12, 3, 1).unwrap(),
                    nonce: "fedcba9876543210fedcba9876543210",
                }));
            }
            app.handle_input(Input::Character('P'));
            let CommentEffect::Load { request_id, .. } = app.take_comment_effects().remove(0)
            else {
                panic!()
            };
            app.apply_comment_result(CommentResult {
                request_id,
                key,
                target: Some(target.clone()),
                outcome: CommentResultOutcome::Loaded(Ok(ExistingComments {
                    comments: Vec::new(),
                    complete,
                    marker_found: false,
                })),
            });
            assert_eq!(
                app.drafts.get(&target).unwrap().submission.is_none(),
                unlocked
            );
            if unlocked {
                app.handle_input(Input::Character('P'));
                app.handle_input(Input::Character('y'));
                let CommentEffect::Publish {
                    body_with_marker, ..
                } = app.take_comment_effects().remove(0)
                else {
                    panic!()
                };
                assert!(body_with_marker.contains("fedcba9876543210fedcba9876543210"));
                assert!(!body_with_marker.contains("0123456789abcdef0123456789abcdef"));
            }
        }
    }

    #[test]
    fn blank_editor_result_has_no_publishable_draft_and_comments_have_loading_empty_failure_states()
    {
        let mut app = App::new(DemoFixture::load());
        app.handle_input(Input::Character('l'));
        app.handle_input(Input::Character('P'));
        assert!(app.status().contains("No saved"));
        app.handle_input(Input::Character('C'));
        assert!(matches!(app.mode(), Mode::Comments { .. }));
        assert!(matches!(
            app.current_comments(),
            Some(CommentListState::Loading)
        ));
        let CommentEffect::Load {
            request_id, key, ..
        } = app.take_comment_effects().remove(0)
        else {
            panic!()
        };
        app.apply_comment_result(CommentResult {
            request_id,
            key: key.clone(),
            target: None,
            outcome: CommentResultOutcome::Loaded(Ok(ExistingComments {
                comments: Vec::new(),
                complete: true,
                marker_found: false,
            })),
        });
        assert!(
            matches!(app.current_comments(), Some(CommentListState::Loaded(list)) if list.comments.is_empty())
        );
        app.handle_input(Input::Character('r'));
        let CommentEffect::Load { request_id, .. } = app.take_comment_effects().remove(0) else {
            panic!()
        };
        app.apply_comment_result(CommentResult {
            request_id,
            key,
            target: None,
            outcome: CommentResultOutcome::Loaded(Err(CommentFailure {
                kind: crate::github::CommentFailureKind::Api,
                http_status: Some(500),
            })),
        });
        assert!(matches!(
            app.current_comments(),
            Some(CommentListState::Failed(_))
        ));
    }

    #[test]
    fn restart_with_submission_is_visibly_locked_and_quit_or_selection_change_never_reposts() {
        let store = MemoryDraftStore::default();
        let inbox = DemoFixture::load();
        let target = CommentTarget::commit(
            inbox.repositories[0].identity.id,
            &inbox.repositories[0].commits[0].sha,
        )
        .unwrap();
        store
            .save(&CommentDraft::new(target.clone(), "durable pending body").unwrap())
            .unwrap();
        store
            .begin_submission(
                &target,
                SubmissionAttempt::new(
                    "0123456789abcdef0123456789abcdef",
                    Utc.with_ymd_and_hms(2024, 1, 15, 12, 0, 0).unwrap(),
                )
                .unwrap(),
            )
            .unwrap();
        let mut app = App::with_stores(
            inbox,
            Box::new(MemoryReviewStore::default()),
            Box::new(store),
        );
        assert!(app.status().contains("unverified"));
        app.handle_input(Input::Character('l'));
        app.handle_input(Input::Character('c'));
        assert_eq!(app.mode(), Mode::Normal);
        app.handle_input(Input::Character('E'));
        assert!(app.take_editor_requests().is_empty());
        app.handle_input(Input::Character('P'));
        assert!(matches!(
            app.take_comment_effects().as_slice(),
            [CommentEffect::Load { .. }]
        ));

        let mut publishing = app_with_commit_draft();
        let _ = begin_publish(&mut publishing);
        publishing.handle_input(Input::Character('j'));
        assert!(
            publishing.take_comment_effects().is_empty(),
            "selection changes must not cancel or repeat a publish"
        );
        publishing.handle_input(Input::Quit);
        assert!(matches!(
            publishing.take_comment_effects().as_slice(),
            [CommentEffect::Cancel { .. }]
        ));
        assert!(
            publishing
                .drafts
                .iter()
                .any(|draft| draft.submission.is_some())
        );
    }

    #[test]
    fn changing_commits_cancels_only_comment_loading_and_stale_load_is_ignored() {
        let mut app = App::new(DemoFixture::load());
        app.handle_input(Input::Character('l'));
        app.handle_input(Input::Character('C'));
        let CommentEffect::Load {
            request_id, key, ..
        } = app.take_comment_effects().remove(0)
        else {
            panic!()
        };
        app.handle_input(Input::Escape);
        app.handle_input(Input::Character('j'));
        assert!(
            matches!(app.take_comment_effects().as_slice(), [CommentEffect::Cancel { request_id: id }] if *id == request_id)
        );
        app.apply_comment_result(CommentResult {
            request_id,
            key,
            target: None,
            outcome: CommentResultOutcome::Loaded(Ok(ExistingComments {
                comments: Vec::new(),
                complete: true,
                marker_found: false,
            })),
        });
        assert!(app.current_comments().is_none());
        assert!(!app.should_quit());
    }
}
