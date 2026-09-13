use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::ffi::OsString;
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};

const FORMAT_VERSION: u64 = 1;
const APPLICATION_DIRECTORY: &str = "reviewbox";
const STATE_FILE_NAME: &str = "review-state.json";
static NEXT_TEMP_FILE: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReviewKey {
    repository_id: u64,
    sha: String,
}

impl ReviewKey {
    pub fn new(repository_id: u64, sha: impl AsRef<str>) -> Result<Self, ReviewKeyError> {
        let sha = sha.as_ref();
        if !is_full_sha(sha) {
            return Err(ReviewKeyError);
        }

        Ok(Self {
            repository_id,
            sha: sha.to_ascii_lowercase(),
        })
    }

    pub fn repository_id(&self) -> u64 {
        self.repository_id
    }

    pub fn sha(&self) -> &str {
        &self.sha
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ReviewKeyError;

impl fmt::Display for ReviewKeyError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("commit identity must be a complete hexadecimal SHA")
    }
}

impl std::error::Error for ReviewKeyError {}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ReviewMarks {
    repositories: BTreeMap<u64, BTreeSet<String>>,
}

impl ReviewMarks {
    pub fn is_reviewed(&self, key: &ReviewKey) -> bool {
        self.repositories
            .get(&key.repository_id)
            .is_some_and(|reviewed| reviewed.contains(&key.sha))
    }

    pub fn reviewed_count(&self) -> usize {
        self.repositories.values().map(BTreeSet::len).sum()
    }

    pub fn repository_count(&self) -> usize {
        self.repositories.len()
    }

    fn set_reviewed(&mut self, key: &ReviewKey, reviewed: bool) {
        if reviewed {
            self.repositories
                .entry(key.repository_id)
                .or_default()
                .insert(key.sha.clone());
            return;
        }

        if let Some(repository) = self.repositories.get_mut(&key.repository_id) {
            repository.remove(&key.sha);
            if repository.is_empty() {
                self.repositories.remove(&key.repository_id);
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReviewStateError {
    Unavailable,
    Read(io::ErrorKind),
    Malformed,
    UnsupportedVersion,
    Write(io::ErrorKind),
}

impl fmt::Display for ReviewStateError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unavailable => formatter.write_str(
                "review state is unavailable; set XDG_DATA_HOME or HOME to an absolute directory",
            ),
            Self::Read(kind) => write!(
                formatter,
                "could not read review state ({kind}); check user-data directory permissions"
            ),
            Self::Malformed => formatter.write_str(
                "review state is malformed; repair or move review-state.json before saving progress",
            ),
            Self::UnsupportedVersion => formatter.write_str(
                "review state uses an unsupported version; upgrade ReviewBox or move review-state.json",
            ),
            Self::Write(kind) => write!(
                formatter,
                "could not save review state ({kind}); check user-data directory permissions and free space"
            ),
        }
    }
}

impl std::error::Error for ReviewStateError {}

pub trait ReviewStore: fmt::Debug {
    fn load(&self) -> Result<ReviewMarks, ReviewStateError>;

    fn set_reviewed(
        &self,
        key: &ReviewKey,
        reviewed: bool,
    ) -> Result<ReviewMarks, ReviewStateError>;
}

#[derive(Clone, Debug)]
pub struct FileReviewStore {
    path: PathBuf,
    #[cfg(test)]
    fail_before_rename: bool,
}

impl FileReviewStore {
    pub fn from_env(lookup: impl Fn(&str) -> Option<OsString>) -> Result<Self, ReviewStateError> {
        let data_home = lookup("XDG_DATA_HOME")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .or_else(|| {
                lookup("HOME")
                    .filter(|value| !value.is_empty())
                    .map(PathBuf::from)
                    .filter(|path| path.is_absolute())
                    .map(|home| home.join(".local").join("share"))
            })
            .ok_or(ReviewStateError::Unavailable)?;

        Ok(Self::at_absolute(
            data_home.join(APPLICATION_DIRECTORY).join(STATE_FILE_NAME),
        ))
    }

    pub fn from_process_env() -> Result<Self, ReviewStateError> {
        Self::from_env(|name| env::var_os(name))
    }

    pub fn at(path: impl Into<PathBuf>) -> Result<Self, ReviewStateError> {
        let path = path.into();
        if !path.is_absolute() {
            return Err(ReviewStateError::Unavailable);
        }
        Ok(Self::at_absolute(path))
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn at_absolute(path: PathBuf) -> Self {
        Self {
            path,
            #[cfg(test)]
            fail_before_rename: false,
        }
    }

    #[cfg(test)]
    fn failing_before_rename(mut self) -> Self {
        self.fail_before_rename = true;
        self
    }

    fn read_marks(&self) -> Result<ReviewMarks, ReviewStateError> {
        match fs::read(&self.path) {
            Ok(bytes) => decode(&bytes),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(ReviewMarks::default()),
            Err(error) => Err(ReviewStateError::Read(error.kind())),
        }
    }

    fn write_marks(&self, marks: &ReviewMarks) -> Result<(), ReviewStateError> {
        let parent = self
            .path
            .parent()
            .filter(|path| !path.as_os_str().is_empty())
            .ok_or(ReviewStateError::Write(io::ErrorKind::InvalidInput))?;

        create_private_directory(parent).map_err(|error| ReviewStateError::Write(error.kind()))?;

        let bytes = encode(marks)?;
        let (temporary_path, mut temporary_file) = create_private_temp_file(parent)?;
        let result: io::Result<()> = (|| {
            temporary_file.write_all(&bytes)?;
            temporary_file.sync_all()?;
            drop(temporary_file);

            #[cfg(test)]
            if self.fail_before_rename {
                return Err(io::Error::other("injected review-state write failure"));
            }

            fs::rename(&temporary_path, &self.path)?;
            sync_directory_best_effort(parent);
            Ok(())
        })();

        if let Err(error) = result {
            let _ = fs::remove_file(&temporary_path);
            return Err(ReviewStateError::Write(error.kind()));
        }

        Ok(())
    }
}

impl ReviewStore for FileReviewStore {
    fn load(&self) -> Result<ReviewMarks, ReviewStateError> {
        self.read_marks()
    }

    fn set_reviewed(
        &self,
        key: &ReviewKey,
        reviewed: bool,
    ) -> Result<ReviewMarks, ReviewStateError> {
        let mut marks = self.read_marks()?;
        marks.set_reviewed(key, reviewed);
        self.write_marks(&marks)?;
        Ok(marks)
    }
}

#[derive(Debug, Default)]
pub struct MemoryReviewStore {
    marks: RefCell<ReviewMarks>,
}

impl MemoryReviewStore {
    pub fn new(marks: ReviewMarks) -> Self {
        Self {
            marks: RefCell::new(marks),
        }
    }
}

impl ReviewStore for MemoryReviewStore {
    fn load(&self) -> Result<ReviewMarks, ReviewStateError> {
        Ok(self.marks.borrow().clone())
    }

    fn set_reviewed(
        &self,
        key: &ReviewKey,
        reviewed: bool,
    ) -> Result<ReviewMarks, ReviewStateError> {
        let mut marks = self.marks.borrow_mut();
        marks.set_reviewed(key, reviewed);
        Ok(marks.clone())
    }
}

#[derive(Deserialize)]
struct VersionProbe {
    version: serde_json::Value,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct StateFile {
    version: u64,
    repositories: BTreeMap<String, RepositoryState>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct RepositoryState {
    reviewed: Vec<String>,
}

fn decode(bytes: &[u8]) -> Result<ReviewMarks, ReviewStateError> {
    let probe: VersionProbe =
        serde_json::from_slice(bytes).map_err(|_| ReviewStateError::Malformed)?;
    let version = probe.version.as_u64().ok_or(ReviewStateError::Malformed)?;
    if version != FORMAT_VERSION {
        return Err(ReviewStateError::UnsupportedVersion);
    }

    let file: StateFile = serde_json::from_slice(bytes).map_err(|_| ReviewStateError::Malformed)?;
    let mut marks = ReviewMarks::default();
    for (repository_text, repository) in file.repositories {
        let repository_id = repository_text
            .parse::<u64>()
            .ok()
            .filter(|id| id.to_string() == repository_text)
            .ok_or(ReviewStateError::Malformed)?;
        for sha in repository.reviewed {
            let key =
                ReviewKey::new(repository_id, sha).map_err(|_| ReviewStateError::Malformed)?;
            marks.set_reviewed(&key, true);
        }
    }
    Ok(marks)
}

fn encode(marks: &ReviewMarks) -> Result<Vec<u8>, ReviewStateError> {
    let repositories = marks
        .repositories
        .iter()
        .map(|(repository_id, reviewed)| {
            (
                repository_id.to_string(),
                RepositoryState {
                    reviewed: reviewed.iter().cloned().collect(),
                },
            )
        })
        .collect();
    let file = StateFile {
        version: FORMAT_VERSION,
        repositories,
    };
    let mut bytes = serde_json::to_vec_pretty(&file)
        .map_err(|_| ReviewStateError::Write(io::ErrorKind::InvalidData))?;
    bytes.push(b'\n');
    Ok(bytes)
}

fn is_full_sha(sha: &str) -> bool {
    matches!(sha.len(), 40 | 64) && sha.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn create_private_directory(path: &Path) -> io::Result<()> {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(path)
}

fn create_private_temp_file(parent: &Path) -> Result<(PathBuf, File), ReviewStateError> {
    loop {
        let sequence = NEXT_TEMP_FILE.fetch_add(1, Ordering::Relaxed);
        let path = parent.join(format!(
            ".{STATE_FILE_NAME}.tmp-{}-{sequence}",
            std::process::id()
        ));
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        match options.open(&path) {
            Ok(file) => return Ok((path, file)),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(ReviewStateError::Write(error.kind())),
        }
    }
}

#[cfg(unix)]
fn sync_directory_best_effort(path: &Path) {
    if let Ok(directory) = File::open(path) {
        let _ = directory.sync_all();
    }
}

#[cfg(not(unix))]
fn sync_directory_best_effort(_path: &Path) {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::thread;

    static NEXT_TEST_DIRECTORY: AtomicU64 = AtomicU64::new(0);

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new() -> Self {
            let sequence = NEXT_TEST_DIRECTORY.fetch_add(1, Ordering::Relaxed);
            let path = env::temp_dir().join(format!(
                "reviewbox-review-state-{}-{sequence}",
                std::process::id()
            ));
            fs::create_dir(&path).expect("create isolated review-state directory");
            Self(path)
        }

        fn state_path(&self) -> PathBuf {
            self.0.join(APPLICATION_DIRECTORY).join(STATE_FILE_NAME)
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn sha(character: char) -> String {
        std::iter::repeat_n(character, 40).collect()
    }

    fn store(directory: &TestDirectory) -> FileReviewStore {
        FileReviewStore::at(directory.state_path()).expect("absolute test path")
    }

    #[test]
    fn missing_state_is_empty_and_load_does_not_create_files() {
        let directory = TestDirectory::new();
        let marks = store(&directory).load().expect("missing state is valid");

        assert_eq!(marks, ReviewMarks::default());
        assert!(!directory.state_path().exists());
        assert_eq!(fs::read_dir(&directory.0).unwrap().count(), 0);
    }

    #[test]
    fn marks_round_trip_across_fresh_store_instances() {
        let directory = TestDirectory::new();
        let key = ReviewKey::new(912_345, sha('a')).unwrap();

        let saved = store(&directory).set_reviewed(&key, true).unwrap();
        let loaded = store(&directory).load().unwrap();

        assert!(saved.is_reviewed(&key));
        assert_eq!(loaded, saved);
        assert_eq!(loaded.reviewed_count(), 1);
        assert_eq!(loaded.repository_count(), 1);
    }

    #[test]
    fn repository_identity_isolates_identical_shas() {
        let directory = TestDirectory::new();
        let first = ReviewKey::new(1, sha('b')).unwrap();
        let second = ReviewKey::new(2, sha('b')).unwrap();

        store(&directory).set_reviewed(&first, true).unwrap();
        store(&directory).set_reviewed(&second, true).unwrap();
        let marks = store(&directory).set_reviewed(&first, false).unwrap();

        assert!(!marks.is_reviewed(&first));
        assert!(marks.is_reviewed(&second));
        assert_eq!(marks.repository_count(), 1);
    }

    #[test]
    fn full_sha_is_preserved_and_uppercase_is_normalized() {
        let directory = TestDirectory::new();
        let uppercase = "ABCDEF0123456789ABCDEF0123456789ABCDEF01";
        let key = ReviewKey::new(7, uppercase).unwrap();
        store(&directory).set_reviewed(&key, true).unwrap();

        let bytes = fs::read_to_string(directory.state_path()).unwrap();
        let lowercase = uppercase.to_ascii_lowercase();
        let file: StateFile = serde_json::from_str(&bytes).unwrap();
        assert_eq!(key.sha(), lowercase);
        assert_eq!(file.repositories["7"].reviewed, vec![lowercase]);
        assert!(!bytes.contains(uppercase));
        assert!(store(&directory).load().unwrap().is_reviewed(&key));
    }

    #[test]
    fn unmark_survives_restart_and_removes_empty_repository() {
        let directory = TestDirectory::new();
        let key = ReviewKey::new(42, sha('c')).unwrap();
        store(&directory).set_reviewed(&key, true).unwrap();

        let saved = store(&directory).set_reviewed(&key, false).unwrap();
        let restarted = store(&directory).load().unwrap();
        let text = fs::read_to_string(directory.state_path()).unwrap();

        assert_eq!(saved, ReviewMarks::default());
        assert_eq!(restarted, ReviewMarks::default());
        assert!(!text.contains("\"42\""));
    }

    #[test]
    fn serialization_is_sorted_deterministic_and_newline_terminated() {
        let directory = TestDirectory::new();
        let second_repo = ReviewKey::new(2, sha('f')).unwrap();
        let first_sha = ReviewKey::new(11, sha('e')).unwrap();
        let second_sha = ReviewKey::new(11, sha('d')).unwrap();
        let review_store = store(&directory);

        review_store.set_reviewed(&second_repo, true).unwrap();
        review_store.set_reviewed(&first_sha, true).unwrap();
        review_store.set_reviewed(&second_sha, true).unwrap();
        let first_bytes = fs::read(directory.state_path()).unwrap();
        review_store.set_reviewed(&second_sha, true).unwrap();
        let second_bytes = fs::read(directory.state_path()).unwrap();

        assert_eq!(first_bytes, second_bytes);
        assert!(first_bytes.ends_with(b"\n"));
        assert_eq!(
            String::from_utf8(first_bytes).unwrap(),
            format!(
                concat!(
                    "{{\n",
                    "  \"version\": 1,\n",
                    "  \"repositories\": {{\n",
                    "    \"11\": {{\n",
                    "      \"reviewed\": [\n",
                    "        \"{}\",\n",
                    "        \"{}\"\n",
                    "      ]\n",
                    "    }},\n",
                    "    \"2\": {{\n",
                    "      \"reviewed\": [\n",
                    "        \"{}\"\n",
                    "      ]\n",
                    "    }}\n",
                    "  }}\n",
                    "}}\n"
                ),
                sha('d'),
                sha('e'),
                sha('f')
            )
        );
    }

    #[test]
    fn malformed_or_unsupported_files_are_reported_and_never_overwritten() {
        let cases = [
            (b"not json".as_slice(), ReviewStateError::Malformed),
            (
                br#"{"version":2,"repositories":{}}"#.as_slice(),
                ReviewStateError::UnsupportedVersion,
            ),
            (
                br#"{"version":1,"repositories":{},"foreign":true}"#.as_slice(),
                ReviewStateError::Malformed,
            ),
            (
                br#"{"version":1,"repositories":{"8":{"reviewed":[],"foreign":true}}}"#.as_slice(),
                ReviewStateError::Malformed,
            ),
            (
                br#"{"version":1,"repositories":{"8":{"reviewed":["short"]}}}"#.as_slice(),
                ReviewStateError::Malformed,
            ),
            (
                br#"{"version":1,"repositories":{"08":{"reviewed":[]}}}"#.as_slice(),
                ReviewStateError::Malformed,
            ),
        ];

        for (index, (bytes, expected)) in cases.into_iter().enumerate() {
            let directory = TestDirectory::new();
            fs::create_dir(directory.0.join(APPLICATION_DIRECTORY)).unwrap();
            fs::write(directory.state_path(), bytes).unwrap();
            let review_store = store(&directory);
            let key = ReviewKey::new(8, sha('8')).unwrap();

            assert_eq!(review_store.load().unwrap_err(), expected, "case {index}");
            assert_eq!(
                review_store.set_reviewed(&key, true).unwrap_err(),
                expected,
                "case {index}"
            );
            assert_eq!(
                fs::read(directory.state_path()).unwrap(),
                bytes,
                "case {index}"
            );
        }
    }

    #[test]
    fn injected_write_failure_preserves_last_good_file_and_cleans_temp_file() {
        let directory = TestDirectory::new();
        let first = ReviewKey::new(3, sha('1')).unwrap();
        let second = ReviewKey::new(3, sha('2')).unwrap();
        store(&directory).set_reviewed(&first, true).unwrap();
        let before = fs::read(directory.state_path()).unwrap();

        let error = store(&directory)
            .failing_before_rename()
            .set_reviewed(&second, true)
            .unwrap_err();

        assert!(matches!(error, ReviewStateError::Write(_)));
        assert_eq!(fs::read(directory.state_path()).unwrap(), before);
        assert!(store(&directory).load().unwrap().is_reviewed(&first));
        assert_eq!(
            fs::read_dir(directory.0.join(APPLICATION_DIRECTORY))
                .unwrap()
                .count(),
            1,
            "failed replacement must leave no temporary file"
        );
    }

    #[test]
    fn readers_observe_only_complete_atomic_replacements() {
        let directory = TestDirectory::new();
        let path = directory.state_path();
        let key = ReviewKey::new(5, sha('5')).unwrap();
        store(&directory).set_reviewed(&key, false).unwrap();
        let running = Arc::new(AtomicBool::new(true));
        let reader_running = Arc::clone(&running);
        let reader_path = path.clone();
        let reader = thread::spawn(move || {
            while reader_running.load(Ordering::Acquire) {
                let bytes = fs::read(&reader_path).expect("atomic destination remains readable");
                assert!(bytes.ends_with(b"\n"));
                let _: StateFile =
                    serde_json::from_slice(&bytes).expect("destination is always complete JSON");
            }
        });

        for reviewed in (0..50).map(|index| index % 2 == 0) {
            store(&directory).set_reviewed(&key, reviewed).unwrap();
        }
        running.store(false, Ordering::Release);
        reader
            .join()
            .expect("reader did not observe a partial file");
    }

    #[test]
    fn environment_location_uses_only_absolute_xdg_or_home() {
        let directory = TestDirectory::new();
        let xdg = directory.0.join("xdg");
        let home = directory.0.join("home");
        let expected_xdg = xdg.join(APPLICATION_DIRECTORY).join(STATE_FILE_NAME);
        let expected_home = home
            .join(".local")
            .join("share")
            .join(APPLICATION_DIRECTORY)
            .join(STATE_FILE_NAME);

        let absolute = FileReviewStore::from_env(|name| match name {
            "XDG_DATA_HOME" => Some(xdg.clone().into_os_string()),
            "HOME" => Some(home.clone().into_os_string()),
            _ => None,
        })
        .unwrap();
        assert_eq!(absolute.path(), expected_xdg);

        for xdg_value in [OsString::from("relative-data"), OsString::new()] {
            let fallback = FileReviewStore::from_env(|name| match name {
                "XDG_DATA_HOME" => Some(xdg_value.clone()),
                "HOME" => Some(home.clone().into_os_string()),
                _ => None,
            })
            .unwrap();
            assert_eq!(fallback.path(), expected_home);
        }

        assert_eq!(
            FileReviewStore::from_env(|_| None).unwrap_err(),
            ReviewStateError::Unavailable
        );
        assert_eq!(
            FileReviewStore::from_env(|name| (name == "HOME").then(|| "relative".into()))
                .unwrap_err(),
            ReviewStateError::Unavailable
        );
    }

    #[cfg(unix)]
    #[test]
    fn created_directory_and_state_file_are_private() {
        use std::os::unix::fs::PermissionsExt;

        let directory = TestDirectory::new();
        let key = ReviewKey::new(6, sha('6')).unwrap();
        store(&directory).set_reviewed(&key, true).unwrap();

        let application_mode = fs::metadata(directory.0.join(APPLICATION_DIRECTORY))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        let file_mode = fs::metadata(directory.state_path())
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(application_mode, 0o700);
        assert_eq!(file_mode, 0o600);
    }

    #[test]
    fn errors_are_sanitized_and_do_not_echo_paths_or_file_contents() {
        let directory = TestDirectory::new();
        fs::create_dir(directory.0.join(APPLICATION_DIRECTORY)).unwrap();
        let secret = "private repository subject";
        fs::write(directory.state_path(), secret).unwrap();

        let message = store(&directory).load().unwrap_err().to_string();
        assert!(!message.contains(secret));
        assert!(!message.contains(directory.0.to_string_lossy().as_ref()));
        assert!(message.contains("malformed"));
    }

    #[test]
    fn review_keys_accept_only_complete_hex_shas() {
        assert!(ReviewKey::new(1, sha('a')).is_ok());
        assert!(ReviewKey::new(1, std::iter::repeat_n('b', 64).collect::<String>()).is_ok());
        assert!(ReviewKey::new(1, "abc123").is_err());
        assert!(ReviewKey::new(1, std::iter::repeat_n('g', 40).collect::<String>()).is_err());
    }
}
