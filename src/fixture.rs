use std::thread;
use std::time::{Duration, Instant};

use chrono::{DateTime, TimeDelta, TimeZone, Utc};

use crate::github::{
    BranchCursor, BranchEnumeration, CancellationToken, HistoryCoverage, HistoryDiscovery,
    HistoryEvent, HistoryPass, LoadFailure, RepositoryHistory, parse_patch_text,
};
use crate::inbox::{
    ChildPane, Commit, DiffLineKind, FileChange, FileStatus, GitHubAuthor, Inbox, PatchCapReason,
    PatchContent, Repository, RepositoryIdentity,
};
use crate::loader::{HistoryPager, cancelled_failure};
use crate::review_state::{MemoryReviewStore, ReviewKey, ReviewStore};

/// Fictional repository whose first history page is entirely reviewed and
/// whose older pages still hold pending commits from long ago.
pub const VACATION_REPOSITORY_ID: u64 = 9_000_007;
pub const VACATION_PENDING_SUBJECT: &str = "Restore fictional tide tables after a long break";
pub const VACATION_OLDEST_SUBJECT: &str = "Sketch fictional harbor lighting plan";
const DEMO_LOGIN: &str = "fictional-reviewer";

/// Builds the fictional, fully populated inbox used by `--demo` and tests.
/// The application-facing types are owned so live data can use the same panes.
pub struct DemoFixture;

impl DemoFixture {
    pub fn load() -> Inbox {
        Inbox::demo(vec![
            repository(
                9_000_001,
                "fictional-labs",
                "orbit-notes-demo",
                primary_commits(),
            ),
            repository(
                9_000_002,
                "fictional-studio",
                "pixel-garden-demo",
                secondary_commits(),
            ),
            repository(
                9_000_003,
                "fictional-co",
                "clockwork-api-demo",
                single_commit(),
            ),
            repository(
                9_000_004,
                "fictional-works",
                "lantern-map-demo",
                secondary_commits(),
            ),
            repository(
                9_000_005,
                "fictional-lab",
                "paper-comet-demo",
                single_commit(),
            ),
            repository(
                9_000_006,
                "fictional-foundry",
                "quiet-signal-demo",
                primary_commits(),
            ),
        ])
    }
}

impl DemoFixture {
    /// Review marks of the demo backlog, held only in memory. They cover the
    /// whole first history page of the vacation repository.
    pub fn review_store() -> MemoryReviewStore {
        let store = MemoryReviewStore::default();
        let keys = vacation_pages()[0]
            .iter()
            .map(|commit| {
                ReviewKey::new(VACATION_REPOSITORY_ID, &commit.sha)
                    .expect("fictional SHAs are complete")
            })
            .collect::<Vec<_>>();
        store
            .set_reviewed_many(&keys, true)
            .expect("memory store accepts fictional marks");
        store
    }

    /// Paged fictional history: every fixture repository fits one page; the
    /// vacation repository spans three.
    fn history_pages() -> Vec<(RepositoryIdentity, Vec<Vec<Commit>>)> {
        let mut repositories = Self::load()
            .repositories
            .into_iter()
            .map(|repository| {
                let base = Utc.with_ymd_and_hms(2024, 1, 15, 12, 0, 0).unwrap();
                let commits = repository
                    .commits
                    .into_iter()
                    .enumerate()
                    .map(|(index, mut commit)| {
                        // Distinct times keep the fixture order in the
                        // newest-first backlog projection.
                        commit.authored_at = base - TimeDelta::minutes(index as i64);
                        commit
                    })
                    .collect();
                (repository.identity, vec![commits])
            })
            .collect::<Vec<_>>();
        repositories.push((
            RepositoryIdentity {
                id: VACATION_REPOSITORY_ID,
                owner: "fictional-harbor".to_owned(),
                name: "tidepool-archive-demo".to_owned(),
            },
            vacation_pages(),
        ));
        repositories
    }
}

fn dated(mut commit: Commit, authored_at: DateTime<Utc>) -> Commit {
    commit.authored_at = authored_at;
    commit
}

fn vacation_pages() -> Vec<Vec<Commit>> {
    vec![
        vec![
            dated(
                commit(
                    "71de001000000000000000000000000000000000",
                    "Tune fictional buoy colors",
                    small_files(),
                ),
                Utc.with_ymd_and_hms(2024, 1, 12, 9, 0, 0).unwrap(),
            ),
            dated(
                commit(
                    "71de002000000000000000000000000000000000",
                    "Label imaginary pier sections",
                    small_files(),
                ),
                Utc.with_ymd_and_hms(2024, 1, 11, 9, 0, 0).unwrap(),
            ),
        ],
        vec![dated(
            commit(
                "71de003000000000000000000000000000000000",
                VACATION_PENDING_SUBJECT,
                small_files(),
            ),
            Utc.with_ymd_and_hms(2023, 6, 20, 16, 30, 0).unwrap(),
        )],
        vec![dated(
            commit(
                "71de004000000000000000000000000000000000",
                VACATION_OLDEST_SUBJECT,
                small_files(),
            ),
            Utc.with_ymd_and_hms(2022, 11, 3, 8, 15, 0).unwrap(),
        )],
    ]
}

/// In-memory fictional history implementing the same pager interface as the
/// GitHub loader. It never touches the network or the filesystem.
pub struct DemoHistoryPager {
    repositories: Vec<(RepositoryIdentity, Vec<Vec<Commit>>)>,
    page_delay: Duration,
}

impl DemoHistoryPager {
    /// Pages arrive immediately; used by the smoke and tests.
    pub fn instant() -> Self {
        Self {
            repositories: DemoFixture::history_pages(),
            page_delay: Duration::ZERO,
        }
    }

    /// Older pages arrive with a short, cancellable delay so progress and
    /// cancellation are observable in the interactive demo.
    pub fn interactive() -> Self {
        Self {
            page_delay: Duration::from_millis(600),
            ..Self::instant()
        }
    }

    fn wait(&self, cancellation: &CancellationToken) -> bool {
        let deadline = Instant::now() + self.page_delay;
        while Instant::now() < deadline {
            if cancellation.is_cancelled() {
                return false;
            }
            thread::sleep(Duration::from_millis(10));
        }
        !cancellation.is_cancelled()
    }
}

impl HistoryPager for DemoHistoryPager {
    fn discover(
        &mut self,
        cancellation: &CancellationToken,
        emit: &mut dyn FnMut(HistoryEvent),
    ) -> Result<HistoryDiscovery, LoadFailure> {
        if cancellation.is_cancelled() {
            return Err(cancelled_failure(0));
        }
        emit(HistoryEvent::DiscoveryPage {
            page: 1,
            owned_repositories: self.repositories.len(),
        });
        Ok(HistoryDiscovery {
            login: DEMO_LOGIN.to_owned(),
            repositories: self
                .repositories
                .iter()
                .enumerate()
                .map(|(index, (identity, _))| RepositoryHistory::new(identity.clone(), index))
                .collect(),
        })
    }

    fn fetch_next_pages(
        &mut self,
        history: &mut RepositoryHistory,
        _login: &str,
        cancellation: &CancellationToken,
        emit: &mut dyn FnMut(HistoryEvent),
    ) -> Result<HistoryPass, LoadFailure> {
        let repository_index = history.repository_index;
        if cancellation.is_cancelled() {
            return Err(cancelled_failure(repository_index));
        }
        let pages = self
            .repositories
            .iter()
            .find(|(identity, _)| identity.id == history.identity.id)
            .map(|(_, pages)| pages.clone())
            .unwrap_or_default();
        if history.enumeration == BranchEnumeration::Pending {
            history.branches = vec![BranchCursor {
                name: "main".to_owned(),
                next_page: 1,
                exhausted: pages.is_empty(),
                failed: None,
            }];
            history.enumeration = BranchEnumeration::Loaded;
            emit(HistoryEvent::BranchPage {
                repository_index,
                page: 1,
                branches: 1,
            });
        }
        let mut pass = HistoryPass {
            pages_fetched: 0,
            accepted_commits: 0,
            coverage: HistoryCoverage::Complete,
        };
        for branch_index in 0..history.branches.len() {
            let cursor = &history.branches[branch_index];
            if cursor.exhausted || cursor.failed.is_some() {
                continue;
            }
            let page = cursor.next_page;
            // The first page is the fast initial batch; older pages simulate
            // network latency in the interactive demo.
            if page > 1 && !self.wait(cancellation) {
                return Err(cancelled_failure(repository_index));
            }
            let mut accepted = 0;
            for commit in pages.get(page - 1).cloned().unwrap_or_default() {
                if !history.commits.contains_key(&commit.sha) {
                    history.commits.insert(commit.sha.clone(), commit);
                    accepted += 1;
                }
            }
            let cursor = &mut history.branches[branch_index];
            cursor.next_page += 1;
            cursor.exhausted = page >= pages.len();
            pass.pages_fetched += 1;
            pass.accepted_commits += accepted;
            emit(HistoryEvent::HistoryPage {
                repository_index,
                branch_index,
                page,
                accepted_commits: accepted,
                total_commits: history.commits.len(),
            });
        }
        pass.coverage = history.coverage();
        Ok(pass)
    }
}

fn repository(id: u64, owner: &str, name: &str, commits: Vec<Commit>) -> Repository {
    Repository {
        identity: RepositoryIdentity {
            id,
            owner: owner.to_owned(),
            name: name.to_owned(),
        },
        commits,
    }
}

fn commit(sha: &str, subject: &str, files: Vec<FileChange>) -> Commit {
    commit_with_files(sha, subject, ChildPane::Available(files))
}

fn commit_with_files(sha: &str, subject: &str, files: ChildPane<FileChange>) -> Commit {
    Commit {
        sha: sha.to_owned(),
        subject: subject.to_owned(),
        author: GitHubAuthor {
            login: "fictional-reviewer".to_owned(),
        },
        authored_at: Utc.with_ymd_and_hms(2024, 1, 15, 12, 0, 0).unwrap(),
        files,
    }
}

fn primary_commits() -> Vec<Commit> {
    vec![
        commit(
            "a1b2c3d000000000000000000000000000000000",
            "Refine fictional launch screen",
            primary_files(),
        ),
        commit_with_files(
            "b2c3d4e000000000000000000000000000000000",
            "Simulate a fictional oversized response",
            ChildPane::ResponseTruncated,
        ),
        commit(
            "c3d4e5f000000000000000000000000000000000",
            "Add imaginary planet routes",
            primary_files(),
        ),
        commit(
            "d4e5f6a000000000000000000000000000000000",
            "Document offline fixture mode",
            small_files(),
        ),
        commit(
            "e5f6a7b000000000000000000000000000000000",
            "Cover made-up route examples",
            primary_files(),
        ),
        commit(
            "f6a7b8c000000000000000000000000000000000",
            "Polish demonstration labels",
            small_files(),
        ),
    ]
}

fn secondary_commits() -> Vec<Commit> {
    vec![
        commit(
            "13579bd000000000000000000000000000000000",
            "Plant fictional color seeds",
            small_files(),
        ),
        commit(
            "2468ace000000000000000000000000000000000",
            "Arrange sample garden tiles",
            primary_files(),
        ),
    ]
}

fn single_commit() -> Vec<Commit> {
    vec![commit(
        "0decafe000000000000000000000000000000000",
        "Calibrate imaginary clockwork",
        small_files(),
    )]
}

fn primary_files() -> Vec<FileChange> {
    vec![
        file("src/welcome.rs", WELCOME_DIFF),
        file("themes/twilight.toml", THEME_DIFF),
        file("src/routes.rs", ROUTES_DIFF),
        file("tests/routes.rs", TEST_DIFF),
        file("README.md", DOC_DIFF),
        no_patch_file("assets/fictional-orbit-map.bin"),
        capped_file("generated/fictional-catalog.rs"),
        file("notes/empty-placeholder.txt", &[]),
    ]
}

fn small_files() -> Vec<FileChange> {
    vec![
        file("src/lib.rs", ROUTES_DIFF),
        file("tests/demo.rs", TEST_DIFF),
    ]
}

fn file(path: &str, lines: &[&str]) -> FileChange {
    let patch = if lines.is_empty() {
        PatchContent::Empty
    } else {
        parse_patch_text(&lines.join("\n"))
    };
    let additions = patch
        .lines()
        .iter()
        .filter(|line| line.kind == DiffLineKind::Addition)
        .count() as u64;
    let deletions = patch
        .lines()
        .iter()
        .filter(|line| line.kind == DiffLineKind::Deletion)
        .count() as u64;
    FileChange {
        path: path.to_owned(),
        api_path_is_commentable: true,
        previous_path: None,
        status: FileStatus::Modified,
        additions,
        deletions,
        changes: additions + deletions,
        patch,
    }
}

fn no_patch_file(path: &str) -> FileChange {
    FileChange {
        path: path.to_owned(),
        api_path_is_commentable: true,
        previous_path: None,
        status: FileStatus::Modified,
        additions: 0,
        deletions: 0,
        changes: 0,
        patch: PatchContent::NoPatch,
    }
}

fn capped_file(path: &str) -> FileChange {
    let retained = parse_patch_text(
        "@@ -1,2 +1,4 @@\n pub fn catalog() {\n+    add_fictional_entry(\"Aster\");\n+    add_fictional_entry(\"Brindle\");\n }",
    );
    FileChange {
        path: path.to_owned(),
        api_path_is_commentable: true,
        previous_path: None,
        status: FileStatus::Modified,
        additions: 2,
        deletions: 0,
        changes: 2,
        patch: PatchContent::Capped {
            lines: retained.lines().to_vec(),
            omitted_lines: 4_992,
            omitted_bytes: 263_168,
            reason: PatchCapReason::FileLimit,
        },
    }
}

const WELCOME_DIFF: &[&str] = &[
    "@@ -1,12 +1,27 @@",
    " pub fn greeting(name: &str) -> String {",
    "-    format!(\"Hello, {name}\")",
    "+    let heading = \"Welcome aboard\";",
    "+    format!(\"{heading}, {name}!\")",
    " }",
    "+",
    "+pub fn beacon_sequence() -> Vec<&'static str> {",
    "+    vec![",
    "+        \"first fictional beacon\",",
    "+        \"second fictional beacon\",",
    "+        \"third fictional beacon\",",
    "+    ]",
    "+}",
    "+",
    "+pub fn launch_checklist() -> Vec<&'static str> {",
    "+    vec![",
    "+        \"seal the sample hatch\",",
    "+        \"count the paper satellites\",",
    "+        \"tune the imaginary receiver\",",
    "+        \"confirm the painted horizon\",",
    "+        \"wave to the cardboard moon\",",
    "+    ]",
    "+}",
    " ",
    " pub fn version() -> &'static str {",
    "     \"demo-1\"",
    " }",
    "@@ -24,6 +39,22 @@ pub fn launch() -> LaunchState {",
    "     let state = LaunchState::Preparing;",
    "+    record(\"first fictional beacon acknowledged\");",
    "+    record(\"second fictional beacon acknowledged\");",
    "+    record(\"third fictional beacon acknowledged\");",
    "+    record(\"crew manifest checked\");",
    "+    record(\"sample route selected\");",
    "+    record(\"practice countdown started\");",
    "+    record(\"practice countdown paused\");",
    "+    record(\"paper map unfolded\");",
    "+    record(\"fictional weather accepted\");",
    "+    record(\"demonstration telemetry enabled\");",
    "+    record(\"this deliberately long fictional telemetry message demonstrates Unicode-safe soft wrapping across a narrow diff pane without hiding any review content from the reader\");",
    "+    record(\"all systems remain imaginary\");",
    "+    record(\"launch review complete\");",
    "     state",
    " }",
];
const THEME_DIFF: &[&str] = &[
    "@@ -4,9 +4,11 @@",
    " [palette]",
    " background = \"#10151c\"",
    " foreground = \"#d8e2ee\"",
    "-accent = \"#80cbc4\"",
    "+accent = \"#67d4c1\"",
    "+selection = \"#244a52\"",
    " ",
    " [panes]",
    "+focused_border = \"accent\"",
    " inactive_border = \"#53606d\"",
];
const ROUTES_DIFF: &[&str] = &[
    "@@ -18,7 +18,16 @@",
    " pub fn routes() -> Router {",
    "     Router::new()",
    "         .route(\"/health\", get(health))",
    "+        .route(\"/planets\", get(list_planets))",
    " }",
    "+",
    "+async fn list_planets() -> Json<Vec<&'static str>> {",
    "+    Json(vec![",
    "+        \"Aster\",",
    "+        \"Brindle\",",
    "+        \"Cinder\",",
    "+    ])",
    "+}",
];
const TEST_DIFF: &[&str] = &[
    "@@ -0,0 +1,13 @@",
    "+#[test]",
    "+fn fictional_planets_are_stable() {",
    "+    let planets = demo_planets();",
    "+    assert_eq!(planets.len(), 3);",
    "+    assert_eq!(planets[0], \"Aster\");",
    "+}",
    "+",
    "+#[test]",
    "+fn health_is_ready() {",
    "+    assert_eq!(health(), \"ready\");",
    "+}",
];
const DOC_DIFF: &[&str] = &[
    "@@ -2,5 +2,8 @@",
    " # Orbit Notes (fictional demo)",
    " ",
    "+This repository exists only inside ReviewBox fixtures.",
    "+It never performs a network request.",
    "+",
    " Run the example with `cargo run`.",
];

#[cfg(test)]
mod tests {
    use chrono::Datelike;

    use super::*;

    #[test]
    fn demo_fixture_has_owned_nested_navigation_data() {
        let fixture = DemoFixture::load();
        assert!(fixture.repositories.len() > 1);
        assert!(
            fixture
                .repositories
                .iter()
                .all(|repo| !repo.commits.is_empty())
        );
        assert!(
            fixture
                .repositories
                .iter()
                .flat_map(|repo| &repo.commits)
                .all(|commit| {
                    commit.sha.len() == 40
                        && commit.sha.bytes().all(|byte| byte.is_ascii_hexdigit())
                        && !commit.subject.is_empty()
                        && (!commit.files.as_slice().is_empty()
                            || matches!(&commit.files, ChildPane::ResponseTruncated))
                })
        );
        assert!(fixture.child_panes_available());
        assert!(WELCOME_DIFF.len() > 32);
        assert!(
            WELCOME_DIFF
                .iter()
                .filter(|line| line.contains("@@"))
                .count()
                > 1
        );

        let primary = &fixture.repositories[0].commits[0];
        assert!(
            primary
                .files
                .as_slice()
                .iter()
                .any(|file| { matches!(&file.patch, PatchContent::NoPatch) })
        );
        assert!(
            primary
                .files
                .as_slice()
                .iter()
                .any(|file| { matches!(&file.patch, PatchContent::Capped { .. }) })
        );
        assert!(primary.files.as_slice().iter().any(|file| {
            file.patch
                .lines()
                .iter()
                .any(|line| line.text.chars().count() > 160)
        }));
        assert!(matches!(
            &fixture.repositories[0].commits[1].files,
            ChildPane::ResponseTruncated
        ));
    }

    #[test]
    fn demo_history_hides_an_old_pending_commit_behind_a_reviewed_first_page() {
        use crate::review_state::ReviewKey;

        let store = DemoFixture::review_store();
        let marks = store.load().unwrap();
        let mut pager = DemoHistoryPager::instant();
        let cancellation = CancellationToken::default();
        let discovery = pager.discover(&cancellation, &mut |_| {}).unwrap();
        let mut history = discovery
            .repositories
            .into_iter()
            .find(|history| history.identity.id == VACATION_REPOSITORY_ID)
            .unwrap();

        pager
            .fetch_next_pages(&mut history, DEMO_LOGIN, &cancellation, &mut |_| {})
            .unwrap();
        assert_eq!(history.coverage(), HistoryCoverage::MoreAvailable);
        assert!(history.commits.values().all(|commit| {
            marks.is_reviewed(&ReviewKey::new(VACATION_REPOSITORY_ID, &commit.sha).unwrap())
        }));

        pager
            .fetch_next_pages(&mut history, DEMO_LOGIN, &cancellation, &mut |_| {})
            .unwrap();
        let pending = history
            .commits
            .values()
            .find(|commit| commit.subject == VACATION_PENDING_SUBJECT)
            .unwrap();
        assert!(pending.authored_at.year() < 2024);
        assert!(!marks.is_reviewed(&ReviewKey::new(VACATION_REPOSITORY_ID, &pending.sha).unwrap()));

        pager
            .fetch_next_pages(&mut history, DEMO_LOGIN, &cancellation, &mut |_| {})
            .unwrap();
        assert_eq!(history.coverage(), HistoryCoverage::Complete);
        assert_eq!(history.commits.len(), 4);

        let cancelled = CancellationToken::default();
        cancelled.cancel();
        assert!(
            pager
                .fetch_next_pages(&mut history, DEMO_LOGIN, &cancelled, &mut |_| {})
                .is_err()
        );
    }
}
