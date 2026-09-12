#[derive(Debug)]
pub struct DemoFixture {
    pub repositories: &'static [Repository],
}

#[derive(Debug)]
pub struct Repository {
    pub name: &'static str,
    pub commits: &'static [Commit],
}

#[derive(Debug)]
pub struct Commit {
    pub short_id: &'static str,
    pub subject: &'static str,
    pub files: &'static [FileChange],
}

impl Commit {
    pub fn label(&self) -> String {
        format!("{}  {}", self.short_id, self.subject)
    }
}

#[derive(Debug)]
pub struct FileChange {
    pub path: &'static str,
    pub diff_lines: &'static [&'static str],
}

impl DemoFixture {
    pub const fn new(repositories: &'static [Repository]) -> Self {
        Self { repositories }
    }

    pub fn load() -> Self {
        Self::new(REPOSITORIES)
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

const EMPTY_DIFF: &[&str] = &[];

const FILES_PRIMARY: &[FileChange] = &[
    FileChange {
        path: "src/welcome.rs",
        diff_lines: WELCOME_DIFF,
    },
    FileChange {
        path: "themes/twilight.toml",
        diff_lines: THEME_DIFF,
    },
    FileChange {
        path: "src/routes.rs",
        diff_lines: ROUTES_DIFF,
    },
    FileChange {
        path: "tests/routes.rs",
        diff_lines: TEST_DIFF,
    },
    FileChange {
        path: "README.md",
        diff_lines: DOC_DIFF,
    },
    FileChange {
        path: "notes/empty-placeholder.txt",
        diff_lines: EMPTY_DIFF,
    },
];

const FILES_SMALL: &[FileChange] = &[
    FileChange {
        path: "src/lib.rs",
        diff_lines: ROUTES_DIFF,
    },
    FileChange {
        path: "tests/demo.rs",
        diff_lines: TEST_DIFF,
    },
];

const COMMITS_PRIMARY: &[Commit] = &[
    Commit {
        short_id: "a1b2c3d",
        subject: "Refine fictional launch screen",
        files: FILES_PRIMARY,
    },
    Commit {
        short_id: "b2c3d4e",
        subject: "Tune sample twilight palette",
        files: FILES_SMALL,
    },
    Commit {
        short_id: "c3d4e5f",
        subject: "Add imaginary planet routes",
        files: FILES_PRIMARY,
    },
    Commit {
        short_id: "d4e5f6a",
        subject: "Document offline fixture mode",
        files: FILES_SMALL,
    },
    Commit {
        short_id: "e5f6a7b",
        subject: "Cover made-up route examples",
        files: FILES_PRIMARY,
    },
    Commit {
        short_id: "f6a7b8c",
        subject: "Polish demonstration labels",
        files: FILES_SMALL,
    },
];

const COMMITS_SECONDARY: &[Commit] = &[
    Commit {
        short_id: "13579bd",
        subject: "Plant fictional color seeds",
        files: FILES_SMALL,
    },
    Commit {
        short_id: "2468ace",
        subject: "Arrange sample garden tiles",
        files: FILES_PRIMARY,
    },
];

const COMMITS_SINGLE: &[Commit] = &[Commit {
    short_id: "0decafe",
    subject: "Calibrate imaginary clockwork",
    files: FILES_SMALL,
}];

const REPOSITORIES: &[Repository] = &[
    Repository {
        name: "fictional-labs/orbit-notes-demo",
        commits: COMMITS_PRIMARY,
    },
    Repository {
        name: "fictional-studio/pixel-garden-demo",
        commits: COMMITS_SECONDARY,
    },
    Repository {
        name: "fictional-co/clockwork-api-demo",
        commits: COMMITS_SINGLE,
    },
    Repository {
        name: "fictional-works/lantern-map-demo",
        commits: COMMITS_SECONDARY,
    },
    Repository {
        name: "fictional-lab/paper-comet-demo",
        commits: COMMITS_SINGLE,
    },
    Repository {
        name: "fictional-foundry/quiet-signal-demo",
        commits: COMMITS_PRIMARY,
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn demo_fixture_has_nested_navigation_data() {
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
                .flat_map(|repo| repo.commits)
                .all(|commit| !commit.files.is_empty())
        );
        assert!(WELCOME_DIFF.len() > 10);
        assert!(
            fixture
                .repositories
                .iter()
                .all(|repo| repo.name.contains("fictional"))
        );
    }
}
