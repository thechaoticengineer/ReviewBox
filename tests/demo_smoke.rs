use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_TEMP_DIRECTORY: AtomicU64 = AtomicU64::new(0);

struct TestDirectory(PathBuf);

impl TestDirectory {
    fn new() -> Self {
        let sequence = NEXT_TEMP_DIRECTORY.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "reviewbox-demo-smoke-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir(&path).expect("create isolated smoke directory");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn demo_smoke_binary_succeeds_without_authentication_network_or_persistent_writes() {
    let isolated = TestDirectory::new();
    let output = Command::new(env!("CARGO_BIN_EXE_reviewbox"))
        .arg("--demo-smoke")
        .env_clear()
        .env("HOME", isolated.path())
        .env("XDG_CONFIG_HOME", isolated.path())
        .env("XDG_DATA_HOME", isolated.path())
        .env("XDG_STATE_HOME", isolated.path())
        .env("XDG_CACHE_HOME", isolated.path())
        .current_dir(isolated.path())
        .output()
        .expect("launch smoke command");

    assert!(
        output.status.success(),
        "smoke command failed: status={} stderr={}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        "ReviewBox demo smoke: ok (40 frames)"
    );
    assert!(output.stderr.is_empty());
    assert_eq!(
        fs::read_dir(isolated.path())
            .expect("inspect isolated directory")
            .count(),
        0,
        "smoke mode must not write runtime state"
    );
}
