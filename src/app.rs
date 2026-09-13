use std::collections::BTreeMap;

use crate::github::{
    LoadEvent, LoadFailure, LoadProgress, LoadStatus, LoadedRepository, RepositoryCoverage,
};
use crate::inbox::{Commit, DiffLine, FileChange, Inbox, InboxSource, Repository};

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

const MAX_VISIBLE_FAILURES: usize = 3;

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
    viewport_heights: [usize; 4],
    pending_g: bool,
    search_query: String,
    status: String,
    should_quit: bool,
    live: Option<LiveInboxState>,
}

impl App {
    pub fn new(inbox: Inbox) -> Self {
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
            viewport_heights: [0; 4],
            pending_g: false,
            search_query: String::new(),
            status,
            should_quit: false,
            live,
        };
        app.normalize();
        app
    }

    pub fn resize(&mut self, width: u16, height: u16) {
        self.terminal_width = width;
        self.terminal_height = height;
        self.viewport_heights = pane_viewport_heights(width, height);
        self.normalize();
    }

    pub fn apply_load_event(&mut self, event: LoadEvent) {
        if self.live.is_none() {
            return;
        }

        let selected_repository = self.current_repository().map(|value| value.identity.id);
        let selected_commit = self.current_commit().map(|value| value.sha.clone());

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

        if let Some(id) = selected_repository
            && let Some(index) = self
                .inbox
                .repositories
                .iter()
                .position(|repository| repository.identity.id == id)
        {
            self.repositories.selected = index;
            if let Some(sha) = selected_commit
                && let Some(index) = self.inbox.repositories[index]
                    .commits
                    .iter()
                    .position(|commit| commit.sha == sha)
            {
                self.commits.selected = index;
            }
        }
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
            Command::Open => self.focus_next("Opened selected item"),
            Command::Back => self.focus_previous("Returned to parent pane"),
            Command::Unrelated => self.status = "Key has no action in normal mode".to_owned(),
        }
    }

    fn apply_search(&mut self, target: Pane) {
        let query = std::mem::take(&mut self.search_query);
        self.mode = Mode::Normal;
        if query.is_empty() {
            self.status = "Empty search; position unchanged".to_owned();
            return;
        }

        let needle = query.to_lowercase();
        let found = match target {
            Pane::Repository => find_wrapped(
                self.inbox.repositories.len(),
                self.repositories.selected,
                |index| {
                    self.inbox.repositories[index]
                        .display_name()
                        .to_lowercase()
                        .contains(&needle)
                },
            ),
            Pane::Commit => find_wrapped(
                self.current_commits().len(),
                self.commits.selected,
                |index| {
                    self.current_commits()[index]
                        .label()
                        .to_lowercase()
                        .contains(&needle)
                },
            ),
            Pane::File => find_wrapped(self.current_files().len(), self.files.selected, |index| {
                self.current_files()[index]
                    .path
                    .to_lowercase()
                    .contains(&needle)
            }),
            Pane::Diff => {
                find_wrapped(self.current_diff_lines().len(), self.diff_scroll, |index| {
                    self.current_diff_lines()[index]
                        .text
                        .to_lowercase()
                        .contains(&needle)
                })
            }
        };

        if let Some(index) = found {
            match target {
                Pane::Repository => self.select_repository(index),
                Pane::Commit => self.select_commit(index),
                Pane::File => self.select_file(index),
                Pane::Diff => self.diff_scroll = index,
            }
            let length = self.dataset_len(target);
            self.status = format!(
                "Match for '{query}' in {} ({}/{length})",
                target.title(),
                index + 1
            );
        } else {
            self.status = format!("No match for '{query}' in {}", target.title());
        }
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
                    self.inbox.repositories.len(),
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
            Pane::Diff => self.diff_scroll = 0,
        }
        self.status = "Moved to first position".to_owned();
    }

    fn move_to_last(&mut self) {
        match self.focus {
            Pane::Repository => {
                self.select_repository(self.inbox.repositories.len().saturating_sub(1));
            }
            Pane::Commit => {
                self.select_commit(self.current_commits().len().saturating_sub(1));
            }
            Pane::File => {
                self.select_file(self.current_files().len().saturating_sub(1));
            }
            Pane::Diff => self.diff_scroll = self.max_diff_scroll(),
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
        }
    }

    fn select_commit(&mut self, selected: usize) {
        if selected != self.commits.selected {
            self.commits.selected = selected;
            self.files = ListPosition::default();
            self.diff_scroll = 0;
        }
    }

    fn select_file(&mut self, selected: usize) {
        if selected != self.files.selected {
            self.files.selected = selected;
            self.diff_scroll = 0;
        }
    }

    fn normalize(&mut self) {
        normalize_list(
            &mut self.repositories,
            self.inbox.repositories.len(),
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
        self.focus = self.focus.min(self.deepest_meaningful_pane());
    }

    fn deepest_meaningful_pane(&self) -> Pane {
        if self.inbox.repositories.is_empty() || self.current_commits().is_empty() {
            Pane::Repository
        } else if !self.inbox.child_panes_available() || self.current_files().is_empty() {
            Pane::Commit
        } else {
            Pane::Diff
        }
    }

    fn max_diff_scroll(&self) -> usize {
        let capacity = self.viewport_heights[Pane::Diff.index()].max(1);
        self.current_diff_lines().len().saturating_sub(capacity)
    }

    pub fn inbox(&self) -> &Inbox {
        &self.inbox
    }

    /// Kept as a compatibility accessor for the fixture-driven demo tests.
    pub fn fixture(&self) -> &Inbox {
        self.inbox()
    }

    pub fn current_repository(&self) -> Option<&Repository> {
        self.inbox.repositories.get(self.repositories.selected)
    }

    pub fn current_commits(&self) -> &[Commit] {
        self.current_repository()
            .map_or(&[], |repository| repository.commits.as_slice())
    }

    pub fn current_commit(&self) -> Option<&Commit> {
        self.current_commits().get(self.commits.selected)
    }

    pub fn current_files(&self) -> &[FileChange] {
        if self.inbox.child_panes_available() {
            self.current_commit()
                .map_or(&[], |commit| commit.files.as_slice())
        } else {
            &[]
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

    pub fn status(&self) -> &str {
        &self.status
    }

    pub fn should_quit(&self) -> bool {
        self.should_quit
    }

    pub fn live_state(&self) -> Option<&LiveInboxState> {
        self.live.as_ref()
    }

    fn dataset_len(&self, pane: Pane) -> usize {
        match pane {
            Pane::Repository => self.inbox.repositories.len(),
            Pane::Commit => self.current_commits().len(),
            Pane::File => self.current_files().len(),
            Pane::Diff => self.current_diff_lines().len(),
        }
    }
}

fn find_wrapped(
    length: usize,
    current: usize,
    mut matches: impl FnMut(usize) -> bool,
) -> Option<usize> {
    if length == 0 {
        return None;
    }

    let current = current.min(length - 1);
    ((current + 1)..length)
        .chain(0..=current)
        .find(|&index| matches(index))
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

fn pane_viewport_heights(width: u16, height: u16) -> [usize; 4] {
    if width < MIN_FULL_WIDTH || height < MIN_FULL_HEIGHT {
        return [0; 4];
    }

    let pane_height = height.saturating_sub(1);
    let repository_outer = pane_height.saturating_mul(34) / 100;
    let commit_outer = pane_height.saturating_mul(33) / 100;
    let file_outer = pane_height
        .saturating_sub(repository_outer)
        .saturating_sub(commit_outer);
    [
        repository_outer.saturating_sub(2) as usize,
        commit_outer.saturating_sub(2) as usize,
        file_outer.saturating_sub(2) as usize,
        pane_height.saturating_sub(2) as usize,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixture::DemoFixture;
    use crate::inbox::{ChildPane, FileStatus, GitHubAuthor, RepositoryIdentity};
    use chrono::{TimeZone, Utc};

    fn files() -> Vec<FileChange> {
        ["one.rs", "two.rs", "three.rs"]
            .into_iter()
            .map(|path| FileChange {
                path: path.to_owned(),
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
        assert_eq!(app.scroll(Pane::Diff), 4);
        app.apply(Command::MoveDown);
        assert_eq!(app.scroll(Pane::Diff), 4);
        app.apply(Command::HalfPageUp);
        assert_eq!(app.scroll(Pane::Diff), 3);
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
    fn live_inbox_does_not_open_fixture_child_panes() {
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
        app.apply(Command::Open);
        assert_eq!(app.focus(), Pane::Commit);
        app.apply(Command::Open);
        assert_eq!(app.focus(), Pane::Commit);
        assert!(app.current_files().is_empty());
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
        for character in ['h', 'j', 'k', 'l', 'g', 'G', '?', 'q'] {
            app.handle_input(Input::Character(character));
        }

        assert_eq!(
            app.mode(),
            Mode::SearchEntry {
                target: Pane::Repository
            }
        );
        assert_eq!(app.search_query(), "hjklgG?q");
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
}
