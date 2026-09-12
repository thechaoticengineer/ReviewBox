use crate::fixture::{Commit, DemoFixture, FileChange, Repository};

pub const MIN_FULL_WIDTH: u16 = 60;
pub const MIN_FULL_HEIGHT: u16 = 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Normal,
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

#[derive(Debug, Default, Clone, Copy)]
struct ListPosition {
    selected: usize,
    scroll: usize,
}

#[derive(Debug)]
pub struct App {
    fixture: DemoFixture,
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
    status: &'static str,
    should_quit: bool,
}

impl App {
    pub fn new(fixture: DemoFixture) -> Self {
        let mut app = Self {
            fixture,
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
            status: "Offline fictional demo",
            should_quit: false,
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

    pub fn apply(&mut self, command: Command) {
        if command == Command::GPrefix {
            if self.pending_g {
                self.pending_g = false;
                self.apply_normal(Command::GPrefix);
            } else {
                self.pending_g = true;
                self.status = "g: waiting for second g";
            }
            self.normalize();
            return;
        }

        self.pending_g = false;
        match self.mode {
            Mode::Normal => self.apply_normal(command),
        }
        self.normalize();
    }

    fn apply_normal(&mut self, command: Command) {
        match command {
            Command::Quit => {
                self.should_quit = true;
                self.status = "Closing demo";
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
            Command::Unrelated => self.status = "Key has no action in normal mode",
        }
    }

    fn focus_previous(&mut self, status: &'static str) {
        if let Some(previous) = self.focus.previous() {
            self.focus = previous;
            self.status = status;
        } else {
            self.status = "Already at repository pane";
        }
    }

    fn focus_next(&mut self, status: &'static str) {
        if let Some(next) = self.focus.next()
            && next <= self.deepest_meaningful_pane()
        {
            self.focus = next;
            self.status = status;
            return;
        }
        self.status = "No child pane to open";
    }

    fn move_active(&mut self, upward: bool, amount: usize) {
        match self.focus {
            Pane::Repository => {
                let selected = moved_index(
                    self.repositories.selected,
                    self.fixture.repositories.len(),
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
        self.status = if upward { "Moved up" } else { "Moved down" };
    }

    fn move_to_first(&mut self) {
        match self.focus {
            Pane::Repository => self.select_repository(0),
            Pane::Commit => self.select_commit(0),
            Pane::File => self.select_file(0),
            Pane::Diff => self.diff_scroll = 0,
        }
        self.status = "Moved to first position";
    }

    fn move_to_last(&mut self) {
        match self.focus {
            Pane::Repository => {
                self.select_repository(self.fixture.repositories.len().saturating_sub(1));
            }
            Pane::Commit => {
                self.select_commit(self.current_commits().len().saturating_sub(1));
            }
            Pane::File => {
                self.select_file(self.current_files().len().saturating_sub(1));
            }
            Pane::Diff => self.diff_scroll = self.max_diff_scroll(),
        }
        self.status = "Moved to last position";
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
            self.fixture.repositories.len(),
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
        if self.fixture.repositories.is_empty() || self.current_commits().is_empty() {
            Pane::Repository
        } else if self.current_files().is_empty() {
            Pane::Commit
        } else {
            Pane::Diff
        }
    }

    fn max_diff_scroll(&self) -> usize {
        let capacity = self.viewport_heights[Pane::Diff.index()].max(1);
        self.current_diff_lines().len().saturating_sub(capacity)
    }

    pub fn fixture(&self) -> &DemoFixture {
        &self.fixture
    }

    pub fn current_repository(&self) -> Option<&Repository> {
        self.fixture.repositories.get(self.repositories.selected)
    }

    pub fn current_commits(&self) -> &[Commit] {
        self.current_repository()
            .map_or(&[], |repository| repository.commits)
    }

    pub fn current_commit(&self) -> Option<&Commit> {
        self.current_commits().get(self.commits.selected)
    }

    pub fn current_files(&self) -> &[FileChange] {
        self.current_commit().map_or(&[], |commit| commit.files)
    }

    pub fn current_file(&self) -> Option<&FileChange> {
        self.current_files().get(self.files.selected)
    }

    pub fn current_diff_lines(&self) -> &[&'static str] {
        self.current_file().map_or(&[], |file| file.diff_lines)
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
        let length = match pane {
            Pane::Repository => self.fixture.repositories.len(),
            Pane::Commit => self.current_commits().len(),
            Pane::File => self.current_files().len(),
            Pane::Diff => self.current_diff_lines().len(),
        };
        if length == 0 {
            (0, 0)
        } else {
            (self.selected(pane).saturating_add(1).min(length), length)
        }
    }

    pub fn pending_g(&self) -> bool {
        self.pending_g
    }

    pub fn status(&self) -> &'static str {
        self.status
    }

    pub fn should_quit(&self) -> bool {
        self.should_quit
    }
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
    use crate::fixture::{Commit, FileChange, Repository};

    const LINES: &[&str] = &["one", "two", "three", "four", "five", "six"];
    const FILES: &[FileChange] = &[
        FileChange {
            path: "one.rs",
            diff_lines: LINES,
        },
        FileChange {
            path: "two.rs",
            diff_lines: LINES,
        },
        FileChange {
            path: "three.rs",
            diff_lines: LINES,
        },
    ];
    const COMMITS: &[Commit] = &[
        Commit {
            short_id: "1111111",
            subject: "one",
            files: FILES,
        },
        Commit {
            short_id: "2222222",
            subject: "two",
            files: FILES,
        },
        Commit {
            short_id: "3333333",
            subject: "three",
            files: FILES,
        },
    ];
    const REPOSITORIES: &[Repository] = &[
        Repository {
            name: "fictional/one",
            commits: COMMITS,
        },
        Repository {
            name: "fictional/two",
            commits: COMMITS,
        },
        Repository {
            name: "fictional/three",
            commits: COMMITS,
        },
    ];
    const NO_REPOSITORIES: &[Repository] = &[];
    const ONE_REPOSITORY: &[Repository] = &[Repository {
        name: "fictional/only",
        commits: &[],
    }];
    const ONE_FILE: &[FileChange] = &[FileChange {
        path: "only.rs",
        diff_lines: &["only line"],
    }];
    const ONE_COMMIT: &[Commit] = &[Commit {
        short_id: "0000001",
        subject: "only",
        files: ONE_FILE,
    }];
    const ONE_NESTED_REPOSITORY: &[Repository] = &[Repository {
        name: "fictional/only-nested",
        commits: ONE_COMMIT,
    }];

    fn app() -> App {
        let mut app = App::new(DemoFixture::new(REPOSITORIES));
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
            assert!(app.selected(Pane::Repository) < app.fixture.repositories.len());
            assert!(app.selected(Pane::Commit) < app.current_commits().len());
            assert!(app.selected(Pane::File) < app.current_files().len());
            assert!(app.scroll(Pane::Diff) <= app.max_diff_scroll());
        }
    }

    #[test]
    fn empty_and_singleton_collections_never_underflow() {
        for repositories in [NO_REPOSITORIES, ONE_REPOSITORY, ONE_NESTED_REPOSITORY] {
            let mut app = App::new(DemoFixture::new(repositories));
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
    fn quit_is_explicit() {
        let mut app = app();
        app.apply(Command::Quit);
        assert!(app.should_quit());
    }
}
