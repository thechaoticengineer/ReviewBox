use chrono::{TimeZone, Utc};

use crate::inbox::{ChildPane, Commit, FileChange, GitHubAuthor, Inbox, Repository};

/// Builds the fictional, fully populated inbox used by `--demo` and tests.
/// The application-facing types are owned so live data can use the same panes.
pub struct DemoFixture;

impl DemoFixture {
    pub fn load() -> Inbox {
        Inbox::demo(vec![
            repository("fictional-labs/orbit-notes-demo", primary_commits()),
            repository("fictional-studio/pixel-garden-demo", secondary_commits()),
            repository("fictional-co/clockwork-api-demo", single_commit()),
            repository("fictional-works/lantern-map-demo", secondary_commits()),
            repository("fictional-lab/paper-comet-demo", single_commit()),
            repository("fictional-foundry/quiet-signal-demo", primary_commits()),
        ])
    }
}

fn repository(name: &str, commits: Vec<Commit>) -> Repository {
    Repository {
        name: name.to_owned(),
        commits,
    }
}

fn commit(sha: &str, subject: &str, files: Vec<FileChange>) -> Commit {
    Commit {
        sha: sha.to_owned(),
        subject: subject.to_owned(),
        author: GitHubAuthor {
            login: "fictional-reviewer".to_owned(),
        },
        authored_at: Utc.with_ymd_and_hms(2024, 1, 15, 12, 0, 0).unwrap(),
        files: ChildPane::Available(files),
    }
}

fn primary_commits() -> Vec<Commit> {
    vec![
        commit(
            "a1b2c3d00000000000000000000000000000000",
            "Refine fictional launch screen",
            primary_files(),
        ),
        commit(
            "b2c3d4e00000000000000000000000000000000",
            "Tune sample twilight palette",
            small_files(),
        ),
        commit(
            "c3d4e5f00000000000000000000000000000000",
            "Add imaginary planet routes",
            primary_files(),
        ),
        commit(
            "d4e5f6a00000000000000000000000000000000",
            "Document offline fixture mode",
            small_files(),
        ),
        commit(
            "e5f6a7b00000000000000000000000000000000",
            "Cover made-up route examples",
            primary_files(),
        ),
        commit(
            "f6a7b8c00000000000000000000000000000000",
            "Polish demonstration labels",
            small_files(),
        ),
    ]
}

fn secondary_commits() -> Vec<Commit> {
    vec![
        commit(
            "13579bd00000000000000000000000000000000",
            "Plant fictional color seeds",
            small_files(),
        ),
        commit(
            "2468ace00000000000000000000000000000000",
            "Arrange sample garden tiles",
            primary_files(),
        ),
    ]
}

fn single_commit() -> Vec<Commit> {
    vec![commit(
        "0decafe00000000000000000000000000000000",
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
    FileChange {
        path: path.to_owned(),
        diff_lines: ChildPane::Available(lines.iter().map(|line| (*line).to_owned()).collect()),
    }
}

const WELCOME_DIFF: &[&str] = &[
    "@@ -1,8 +1,12 @@",
    " pub fn greeting(name: &str) -> String {",
    "-    format!(\"Hello, {name}\")",
    "+    let heading = \"Welcome aboard\";",
    "+    format!(\"{heading}, {name}!\")",
    " }",
    "+",
    "+#[cfg(test)]",
    "+mod tests {",
    "+    // Fictional example assertion.",
    "+}",
    " ",
    " pub fn version() -> &'static str {",
    "     \"demo-1\"",
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
                    !commit.sha.is_empty()
                        && !commit.subject.is_empty()
                        && !commit.files.as_slice().is_empty()
                })
        );
        assert!(fixture.child_panes_available());
        assert!(WELCOME_DIFF.len() > 10);
    }
}
