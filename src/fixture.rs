use chrono::{TimeZone, Utc};

use crate::github::parse_patch_text;
use crate::inbox::{
    ChildPane, Commit, DiffLineKind, FileChange, FileStatus, GitHubAuthor, Inbox, PatchCapReason,
    PatchContent, Repository, RepositoryIdentity,
};

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
}
