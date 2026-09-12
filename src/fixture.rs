#[derive(Debug)]
pub struct DemoFixture {
    pub repository: &'static str,
    pub commit: &'static str,
    pub file: &'static str,
    pub diff_lines: &'static [&'static str],
}

impl DemoFixture {
    pub fn load() -> Self {
        Self {
            repository: "example-labs/orbit-notes",
            commit: "a1b2c3d  Refine fictional launch screen",
            file: "src/welcome.rs",
            diff_lines: &[
                "@@ -1,3 +1,4 @@",
                " pub fn greeting() -> &'static str {",
                "-    \"hello\"",
                "+    \"hello from the ReviewBox demo\"",
                " }",
            ],
        }
    }
}
