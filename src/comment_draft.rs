use std::cell::RefCell;
use std::collections::BTreeMap;
use std::env;
use std::ffi::OsString;
use std::fmt;
use std::fs::File;
use std::io::{self, Read};
use std::marker::PhantomData;
use std::path::{Path, PathBuf};

use serde::de::{MapAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize};

use crate::inbox::{DiffLineKind, FileChange, PatchCapReason, PatchContent};
use crate::private_file::{atomic_replace, data_file_path};
use crate::review_state::ReviewKey;

const FORMAT_VERSION: u64 = 1;
const STATE_FILE_NAME: &str = "comment-drafts.json";
const MAX_STATE_BYTES: u64 = 16 * 1024 * 1024;
pub const MAX_DRAFT_CHARACTERS: usize = 65_000;
pub const MAX_PATH_BYTES: usize = 4_096;

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct LineTarget {
    pub path: String,
    pub position: u32,
}

impl LineTarget {
    pub fn new(path: impl Into<String>, position: u32) -> Result<Self, DraftStateError> {
        let target = Self {
            path: path.into(),
            position,
        };
        validate_line_target(&target)?;
        Ok(target)
    }
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum CommentAnchor {
    Commit,
    Line(LineTarget),
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct CommentTarget {
    repository_id: u64,
    sha: String,
    anchor: CommentAnchor,
}

impl CommentTarget {
    pub fn new(
        repository_id: u64,
        sha: impl AsRef<str>,
        anchor: CommentAnchor,
    ) -> Result<Self, DraftStateError> {
        let key = ReviewKey::new(repository_id, sha).map_err(|_| DraftStateError::InvalidDraft)?;
        if let CommentAnchor::Line(line) = &anchor {
            validate_line_target(line)?;
        }
        Ok(Self {
            repository_id: key.repository_id(),
            sha: key.sha().to_owned(),
            anchor,
        })
    }

    pub fn commit(repository_id: u64, sha: impl AsRef<str>) -> Result<Self, DraftStateError> {
        Self::new(repository_id, sha, CommentAnchor::Commit)
    }

    pub fn line(
        repository_id: u64,
        sha: impl AsRef<str>,
        line: LineTarget,
    ) -> Result<Self, DraftStateError> {
        Self::new(repository_id, sha, CommentAnchor::Line(line))
    }

    pub fn repository_id(&self) -> u64 {
        self.repository_id
    }

    pub fn sha(&self) -> &str {
        &self.sha
    }

    pub fn anchor(&self) -> &CommentAnchor {
        &self.anchor
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CommentDraft {
    pub target: CommentTarget,
    pub body: String,
}

impl CommentDraft {
    pub fn new(target: CommentTarget, body: impl Into<String>) -> Result<Self, DraftStateError> {
        let draft = Self {
            target,
            body: body.into(),
        };
        validate_draft(&draft)?;
        Ok(draft)
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CommentDrafts {
    drafts: BTreeMap<CommentTarget, CommentDraft>,
}

impl CommentDrafts {
    pub fn get(&self, target: &CommentTarget) -> Option<&CommentDraft> {
        self.drafts.get(target)
    }

    pub fn iter(&self) -> impl Iterator<Item = &CommentDraft> {
        self.drafts.values()
    }

    pub fn len(&self) -> usize {
        self.drafts.len()
    }

    pub fn is_empty(&self) -> bool {
        self.drafts.is_empty()
    }

    fn insert(&mut self, draft: CommentDraft) {
        self.drafts.insert(draft.target.clone(), draft);
    }

    fn remove(&mut self, target: &CommentTarget) {
        self.drafts.remove(target);
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DraftStateError {
    Unavailable,
    Read(io::ErrorKind),
    Malformed,
    UnsupportedVersion,
    Write(io::ErrorKind),
    InvalidDraft,
}

impl fmt::Display for DraftStateError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unavailable => formatter.write_str(
                "comment-drafts.json is unavailable; set XDG_DATA_HOME or HOME to an absolute directory",
            ),
            Self::Read(kind) => write!(
                formatter,
                "could not read comment-drafts.json ({kind}); check user-data directory permissions"
            ),
            Self::Malformed => formatter.write_str(
                "comment drafts are malformed; repair or move comment-drafts.json before saving drafts",
            ),
            Self::UnsupportedVersion => formatter.write_str(
                "comment drafts use an unsupported version; upgrade ReviewBox or move comment-drafts.json",
            ),
            Self::Write(kind) => write!(
                formatter,
                "could not save comment-drafts.json ({kind}); check user-data directory permissions and free space"
            ),
            Self::InvalidDraft => formatter.write_str(
                "comment-drafts.json rejected an invalid draft identity, target, or body",
            ),
        }
    }
}

impl std::error::Error for DraftStateError {}

pub trait DraftStore: fmt::Debug {
    fn load(&self) -> Result<CommentDrafts, DraftStateError>;
    fn save(&self, draft: &CommentDraft) -> Result<CommentDrafts, DraftStateError>;
    fn delete(&self, target: &CommentTarget) -> Result<CommentDrafts, DraftStateError>;
}

#[derive(Clone, Debug)]
pub struct FileDraftStore {
    path: PathBuf,
    #[cfg(test)]
    fail_before_rename: bool,
}

impl FileDraftStore {
    pub fn from_env(lookup: impl Fn(&str) -> Option<OsString>) -> Result<Self, DraftStateError> {
        let path = data_file_path(lookup, STATE_FILE_NAME).ok_or(DraftStateError::Unavailable)?;
        Ok(Self::at_absolute(path))
    }

    pub fn from_process_env() -> Result<Self, DraftStateError> {
        Self::from_env(|name| env::var_os(name))
    }

    pub fn at(path: impl Into<PathBuf>) -> Result<Self, DraftStateError> {
        let path = path.into();
        if !path.is_absolute() {
            return Err(DraftStateError::Unavailable);
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

    fn read_drafts(&self) -> Result<CommentDrafts, DraftStateError> {
        let mut file = match File::open(&self.path) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Ok(CommentDrafts::default());
            }
            Err(error) => return Err(DraftStateError::Read(error.kind())),
        };
        if file
            .metadata()
            .map_err(|error| DraftStateError::Read(error.kind()))?
            .len()
            > MAX_STATE_BYTES
        {
            return Err(DraftStateError::Malformed);
        }
        let mut bytes = Vec::new();
        file.by_ref()
            .take(MAX_STATE_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|error| DraftStateError::Read(error.kind()))?;
        if bytes.len() as u64 > MAX_STATE_BYTES {
            return Err(DraftStateError::Malformed);
        }
        decode(&bytes)
    }

    fn write_drafts(&self, drafts: &CommentDrafts) -> Result<(), DraftStateError> {
        let bytes = encode(drafts)?;
        atomic_replace(&self.path, STATE_FILE_NAME, &bytes, || {
            #[cfg(test)]
            if self.fail_before_rename {
                return Err(io::Error::other("injected comment-draft write failure"));
            }
            Ok(())
        })
        .map_err(|error| DraftStateError::Write(error.kind()))
    }
}

impl DraftStore for FileDraftStore {
    fn load(&self) -> Result<CommentDrafts, DraftStateError> {
        self.read_drafts()
    }

    fn save(&self, draft: &CommentDraft) -> Result<CommentDrafts, DraftStateError> {
        validate_draft(draft)?;
        let mut drafts = self.read_drafts()?;
        drafts.insert(draft.clone());
        self.write_drafts(&drafts)?;
        Ok(drafts)
    }

    fn delete(&self, target: &CommentTarget) -> Result<CommentDrafts, DraftStateError> {
        validate_target(target)?;
        let mut drafts = self.read_drafts()?;
        drafts.remove(target);
        self.write_drafts(&drafts)?;
        Ok(drafts)
    }
}

#[derive(Debug, Default)]
pub struct MemoryDraftStore {
    drafts: RefCell<CommentDrafts>,
}

impl MemoryDraftStore {
    pub fn new(drafts: CommentDrafts) -> Self {
        Self {
            drafts: RefCell::new(drafts),
        }
    }
}

impl DraftStore for MemoryDraftStore {
    fn load(&self) -> Result<CommentDrafts, DraftStateError> {
        Ok(self.drafts.borrow().clone())
    }

    fn save(&self, draft: &CommentDraft) -> Result<CommentDrafts, DraftStateError> {
        validate_draft(draft)?;
        let mut drafts = self.drafts.borrow_mut();
        drafts.insert(draft.clone());
        Ok(drafts.clone())
    }

    fn delete(&self, target: &CommentTarget) -> Result<CommentDrafts, DraftStateError> {
        validate_target(target)?;
        let mut drafts = self.drafts.borrow_mut();
        drafts.remove(target);
        Ok(drafts.clone())
    }
}

pub fn line_target(file: &FileChange, row: usize) -> Option<LineTarget> {
    if !file.api_path_is_commentable {
        return None;
    }
    let lines = match &file.patch {
        PatchContent::Text { lines } => lines,
        PatchContent::Capped {
            lines,
            reason: PatchCapReason::FileLimit,
            ..
        } => lines,
        PatchContent::Capped { .. }
        | PatchContent::Empty
        | PatchContent::NoPatch
        | PatchContent::Unavailable => return None,
    };
    if lines.first().map(|line| line.kind) != Some(DiffLineKind::Hunk) || row == 0 {
        return None;
    }
    if !matches!(
        lines.get(row)?.kind,
        DiffLineKind::Context | DiffLineKind::Addition | DiffLineKind::Deletion
    ) {
        return None;
    }
    LineTarget::new(file.path.clone(), u32::try_from(row).ok()?).ok()
}

fn validate_line_target(target: &LineTarget) -> Result<(), DraftStateError> {
    if target.path.is_empty()
        || target.path.len() > MAX_PATH_BYTES
        || target.path.chars().any(char::is_control)
        || target.position == 0
    {
        return Err(DraftStateError::InvalidDraft);
    }
    Ok(())
}

fn validate_target(target: &CommentTarget) -> Result<(), DraftStateError> {
    ReviewKey::new(target.repository_id, &target.sha).map_err(|_| DraftStateError::InvalidDraft)?;
    if let CommentAnchor::Line(line) = &target.anchor {
        validate_line_target(line)?;
    }
    Ok(())
}

fn validate_draft(draft: &CommentDraft) -> Result<(), DraftStateError> {
    validate_target(&draft.target)?;
    if !draft
        .body
        .chars()
        .any(|character| !character.is_whitespace())
        || draft.body.chars().count() > MAX_DRAFT_CHARACTERS
    {
        return Err(DraftStateError::InvalidDraft);
    }
    Ok(())
}

#[derive(Deserialize)]
struct VersionProbe {
    version: serde_json::Value,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct DraftFile {
    version: u64,
    repositories: StrictMap<RepositoryDrafts>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct RepositoryDrafts {
    commits: StrictMap<CommitDrafts>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct CommitDrafts {
    #[serde(skip_serializing_if = "Option::is_none")]
    commit: Option<BodyDraft>,
    lines: Vec<LineDraft>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct BodyDraft {
    body: String,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct LineDraft {
    path: String,
    position: u32,
    body: String,
}

#[derive(Serialize)]
#[serde(transparent)]
struct StrictMap<T>(BTreeMap<String, T>);

impl<'de, T> Deserialize<'de> for StrictMap<T>
where
    T: Deserialize<'de>,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct StrictMapVisitor<T>(PhantomData<T>);

        impl<'de, T> Visitor<'de> for StrictMapVisitor<T>
        where
            T: Deserialize<'de>,
        {
            type Value = StrictMap<T>;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("an object without duplicate keys")
            }

            fn visit_map<A>(self, mut access: A) -> Result<Self::Value, A::Error>
            where
                A: MapAccess<'de>,
            {
                let mut values = BTreeMap::new();
                while let Some((key, value)) = access.next_entry::<String, T>()? {
                    if values.insert(key, value).is_some() {
                        return Err(serde::de::Error::custom("duplicate map key"));
                    }
                }
                Ok(StrictMap(values))
            }
        }

        deserializer.deserialize_map(StrictMapVisitor(PhantomData))
    }
}

fn decode(bytes: &[u8]) -> Result<CommentDrafts, DraftStateError> {
    let probe: VersionProbe =
        serde_json::from_slice(bytes).map_err(|_| DraftStateError::Malformed)?;
    let version = probe.version.as_u64().ok_or(DraftStateError::Malformed)?;
    if version != FORMAT_VERSION {
        return Err(DraftStateError::UnsupportedVersion);
    }

    let file: DraftFile = serde_json::from_slice(bytes).map_err(|_| DraftStateError::Malformed)?;
    let mut drafts = CommentDrafts::default();
    for (repository_text, repository) in file.repositories.0 {
        let repository_id = repository_text
            .parse::<u64>()
            .ok()
            .filter(|id| id.to_string() == repository_text)
            .ok_or(DraftStateError::Malformed)?;
        for (sha, commit) in repository.commits.0 {
            let base = CommentTarget::commit(repository_id, &sha)
                .map_err(|_| DraftStateError::Malformed)?;
            if base.sha() != sha {
                return Err(DraftStateError::Malformed);
            }
            if let Some(body) = commit.commit {
                insert_decoded(
                    &mut drafts,
                    CommentDraft {
                        target: base,
                        body: body.body,
                    },
                )?;
            }
            for line in commit.lines {
                let body = line.body;
                let line_target = LineTarget {
                    path: line.path,
                    position: line.position,
                };
                let target = CommentTarget::line(repository_id, &sha, line_target)
                    .map_err(|_| DraftStateError::Malformed)?;
                insert_decoded(&mut drafts, CommentDraft { target, body })?;
            }
        }
    }
    Ok(drafts)
}

fn insert_decoded(drafts: &mut CommentDrafts, draft: CommentDraft) -> Result<(), DraftStateError> {
    validate_draft(&draft).map_err(|_| DraftStateError::Malformed)?;
    if drafts.drafts.contains_key(&draft.target) {
        return Err(DraftStateError::Malformed);
    }
    drafts.insert(draft);
    Ok(())
}

fn encode(drafts: &CommentDrafts) -> Result<Vec<u8>, DraftStateError> {
    let mut repositories = BTreeMap::<String, RepositoryDrafts>::new();
    for draft in drafts.iter() {
        validate_draft(draft)?;
        let repository = repositories
            .entry(draft.target.repository_id().to_string())
            .or_insert_with(|| RepositoryDrafts {
                commits: StrictMap(BTreeMap::new()),
            });
        let commit = repository
            .commits
            .0
            .entry(draft.target.sha().to_owned())
            .or_insert_with(|| CommitDrafts {
                commit: None,
                lines: Vec::new(),
            });
        match draft.target.anchor() {
            CommentAnchor::Commit => {
                commit.commit = Some(BodyDraft {
                    body: draft.body.clone(),
                });
            }
            CommentAnchor::Line(line) => commit.lines.push(LineDraft {
                path: line.path.clone(),
                position: line.position,
                body: draft.body.clone(),
            }),
        }
    }
    let file = DraftFile {
        version: FORMAT_VERSION,
        repositories: StrictMap(repositories),
    };
    let mut bytes = serde_json::to_vec_pretty(&file)
        .map_err(|_| DraftStateError::Write(io::ErrorKind::InvalidData))?;
    bytes.push(b'\n');
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};

    use crate::github::parse_patch_text;
    use crate::inbox::{FileStatus, PatchCapReason};

    static NEXT_TEST_DIRECTORY: AtomicU64 = AtomicU64::new(0);

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new() -> Self {
            let sequence = NEXT_TEST_DIRECTORY.fetch_add(1, Ordering::Relaxed);
            let path = env::temp_dir().join(format!(
                "reviewbox-comment-drafts-{}-{sequence}",
                std::process::id()
            ));
            fs::create_dir(&path).expect("create isolated comment-draft directory");
            Self(path)
        }

        fn state_path(&self) -> PathBuf {
            self.0.join("reviewbox").join(STATE_FILE_NAME)
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

    fn store(directory: &TestDirectory) -> FileDraftStore {
        FileDraftStore::at(directory.state_path()).expect("absolute test path")
    }

    fn commit_draft(repository_id: u64, sha: &str, body: &str) -> CommentDraft {
        CommentDraft::new(
            CommentTarget::commit(repository_id, sha).unwrap(),
            body.to_owned(),
        )
        .unwrap()
    }

    fn line_draft(
        repository_id: u64,
        sha: &str,
        path: &str,
        position: u32,
        body: &str,
    ) -> CommentDraft {
        CommentDraft::new(
            CommentTarget::line(repository_id, sha, LineTarget::new(path, position).unwrap())
                .unwrap(),
            body.to_owned(),
        )
        .unwrap()
    }

    #[test]
    fn identities_normalize_sha_and_distinguish_commit_and_line_targets() {
        let uppercase = "ABCDEF0123456789ABCDEF0123456789ABCDEF01";
        let commit = CommentTarget::commit(7, uppercase).unwrap();
        let same_line = LineTarget::new("src/lib.rs", 4).unwrap();
        let line = CommentTarget::line(7, uppercase, same_line.clone()).unwrap();
        let other_repository = CommentTarget::line(8, uppercase, same_line.clone()).unwrap();
        let other_position =
            CommentTarget::line(7, uppercase, LineTarget::new("src/lib.rs", 5).unwrap()).unwrap();
        let other_path =
            CommentTarget::line(7, uppercase, LineTarget::new("src/main.rs", 4).unwrap()).unwrap();

        assert_eq!(commit.sha(), uppercase.to_ascii_lowercase());
        assert_ne!(commit, line);
        assert_ne!(line, other_repository);
        assert_ne!(line, other_position);
        assert_ne!(line, other_path);
        assert_eq!(line.anchor(), &CommentAnchor::Line(same_line));
        assert!(CommentTarget::commit(7, std::iter::repeat_n('a', 64).collect::<String>()).is_ok());
    }

    #[test]
    fn invalid_sha_path_position_and_body_are_rejected() {
        for invalid_sha in ["short", "gggggggggggggggggggggggggggggggggggggggg"] {
            assert_eq!(
                CommentTarget::commit(1, invalid_sha).unwrap_err(),
                DraftStateError::InvalidDraft
            );
        }
        for invalid_line in [
            LineTarget {
                path: String::new(),
                position: 1,
            },
            LineTarget {
                path: "src/bad\tname.rs".to_owned(),
                position: 1,
            },
            LineTarget {
                path: "src/bad\u{7}name.rs".to_owned(),
                position: 1,
            },
            LineTarget {
                path: "a".repeat(MAX_PATH_BYTES + 1),
                position: 1,
            },
            LineTarget {
                path: "src/lib.rs".to_owned(),
                position: 0,
            },
        ] {
            assert_eq!(
                CommentTarget::line(1, sha('a'), invalid_line).unwrap_err(),
                DraftStateError::InvalidDraft
            );
        }

        let target = CommentTarget::commit(1, sha('a')).unwrap();
        for body in [
            String::new(),
            " \n\t".to_owned(),
            "x".repeat(MAX_DRAFT_CHARACTERS + 1),
        ] {
            assert_eq!(
                CommentDraft::new(target.clone(), body).unwrap_err(),
                DraftStateError::InvalidDraft
            );
        }
        assert!(CommentDraft::new(target, "x".repeat(MAX_DRAFT_CHARACTERS)).is_ok());
    }

    #[test]
    fn missing_file_is_empty_and_load_does_not_touch_disk() {
        let directory = TestDirectory::new();
        assert!(store(&directory).load().unwrap().is_empty());
        assert_eq!(fs::read_dir(&directory.0).unwrap().count(), 0);
    }

    #[test]
    fn deterministic_serialization_matches_the_v1_schema_and_rewrites_identically() {
        let directory = TestDirectory::new();
        let review_store = store(&directory);
        let repo_two = commit_draft(2, &sha('f'), "Later repository");
        let repo_eleven_commit = commit_draft(11, &sha('e'), "Commit note");
        let repo_eleven_line = line_draft(11, &sha('d'), "src/lib.rs", 3, "Line note");
        review_store.save(&repo_two).unwrap();
        review_store.save(&repo_eleven_commit).unwrap();
        review_store.save(&repo_eleven_line).unwrap();
        let first = fs::read(directory.state_path()).unwrap();
        review_store.save(&repo_eleven_line).unwrap();
        let second = fs::read(directory.state_path()).unwrap();

        assert_eq!(first, second);
        assert_eq!(
            String::from_utf8(first).unwrap(),
            format!(
                concat!(
                    "{{\n",
                    "  \"version\": 1,\n",
                    "  \"repositories\": {{\n",
                    "    \"11\": {{\n",
                    "      \"commits\": {{\n",
                    "        \"{}\": {{\n",
                    "          \"lines\": [\n",
                    "            {{\n",
                    "              \"path\": \"src/lib.rs\",\n",
                    "              \"position\": 3,\n",
                    "              \"body\": \"Line note\"\n",
                    "            }}\n",
                    "          ]\n",
                    "        }},\n",
                    "        \"{}\": {{\n",
                    "          \"commit\": {{\n",
                    "            \"body\": \"Commit note\"\n",
                    "          }},\n",
                    "          \"lines\": []\n",
                    "        }}\n",
                    "      }}\n",
                    "    }},\n",
                    "    \"2\": {{\n",
                    "      \"commits\": {{\n",
                    "        \"{}\": {{\n",
                    "          \"commit\": {{\n",
                    "            \"body\": \"Later repository\"\n",
                    "          }},\n",
                    "          \"lines\": []\n",
                    "        }}\n",
                    "      }}\n",
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
    fn restart_update_and_delete_preserve_every_unrelated_target() {
        let directory = TestDirectory::new();
        let same_commit = commit_draft(4, &sha('a'), "Commit body");
        let first_line = line_draft(4, &sha('a'), "src/lib.rs", 1, "First line");
        let updated_line = line_draft(4, &sha('a'), "src/lib.rs", 1, "Updated line");
        let second_line = line_draft(4, &sha('a'), "src/lib.rs", 9, "Second line");
        let other_commit = commit_draft(4, &sha('b'), "Other commit");
        let other_repository = commit_draft(5, &sha('a'), "Other repository");
        for draft in [
            &same_commit,
            &first_line,
            &second_line,
            &other_commit,
            &other_repository,
        ] {
            store(&directory).save(draft).unwrap();
        }

        let updated = store(&directory).save(&updated_line).unwrap();
        assert_eq!(updated.len(), 5);
        assert_eq!(
            updated.get(&updated_line.target).unwrap().body,
            "Updated line"
        );
        let after_delete = store(&directory).delete(&same_commit.target).unwrap();
        assert_eq!(after_delete.len(), 4);
        assert!(after_delete.get(&same_commit.target).is_none());
        for retained in [
            &updated_line,
            &second_line,
            &other_commit,
            &other_repository,
        ] {
            assert_eq!(
                after_delete.get(&retained.target).unwrap().body,
                retained.body
            );
        }

        for removed in [&updated_line, &second_line, &other_commit] {
            store(&directory).delete(&removed.target).unwrap();
        }
        let restarted = store(&directory).load().unwrap();
        assert_eq!(restarted.len(), 1);
        assert!(restarted.get(&other_repository.target).is_some());
        let text = fs::read_to_string(directory.state_path()).unwrap();
        assert!(!text.contains("\"4\""));
        assert!(text.contains("\"5\""));
    }

    #[test]
    fn malformed_unsupported_unknown_duplicate_and_invalid_files_are_never_overwritten() {
        let duplicate_sha = sha('a');
        let cases = vec![
            (b"not json".to_vec(), DraftStateError::Malformed),
            (
                br#"{"version":2,"repositories":{}}"#.to_vec(),
                DraftStateError::UnsupportedVersion,
            ),
            (
                br#"{"version":1,"repositories":{},"foreign":true}"#.to_vec(),
                DraftStateError::Malformed,
            ),
            (
                format!(
                    "{{\"version\":1,\"repositories\":{{\"01\":{{\"commits\":{{\"{duplicate_sha}\":{{\"commit\":{{\"body\":\"body\"}},\"lines\":[]}}}}}}}}}}"
                )
                .into_bytes(),
                DraftStateError::Malformed,
            ),
            (
                format!(
                    "{{\"version\":1,\"repositories\":{{\"1\":{{\"commits\":{{\"{duplicate_sha}\":{{\"lines\":[{{\"path\":\"src/lib.rs\",\"position\":1,\"body\":\"one\"}},{{\"path\":\"src/lib.rs\",\"position\":1,\"body\":\"two\"}}]}}}}}}}}}}"
                )
                .into_bytes(),
                DraftStateError::Malformed,
            ),
            (
                format!(
                    "{{\"version\":1,\"repositories\":{{\"1\":{{\"commits\":{{\"{duplicate_sha}\":{{\"commit\":{{\"body\":\"   \"}},\"lines\":[]}}}}}}}}}}"
                )
                .into_bytes(),
                DraftStateError::Malformed,
            ),
            (
                format!(
                    "{{\"version\":1,\"repositories\":{{\"1\":{{\"commits\":{{\"{duplicate_sha}\":{{\"lines\":[{{\"path\":\"src/lib.rs\",\"position\":4294967296,\"body\":\"body\"}}]}}}}}}}}}}"
                )
                .into_bytes(),
                DraftStateError::Malformed,
            ),
            (
                format!(
                    "{{\"version\":1,\"repositories\":{{\"1\":{{\"commits\":{{\"{duplicate_sha}\":{{\"lines\":[]}}}},\"{duplicate_sha}\":{{\"lines\":[]}}}}}}}}}}"
                )
                .into_bytes(),
                DraftStateError::Malformed,
            ),
        ];
        let valid = commit_draft(1, &sha('b'), "Safe replacement");
        for (index, (bytes, expected)) in cases.into_iter().enumerate() {
            let directory = TestDirectory::new();
            fs::create_dir(directory.0.join("reviewbox")).unwrap();
            fs::write(directory.state_path(), &bytes).unwrap();
            let draft_store = store(&directory);
            assert_eq!(
                draft_store.load().unwrap_err(),
                expected,
                "load case {index}"
            );
            assert_eq!(
                draft_store.save(&valid).unwrap_err(),
                expected,
                "save case {index}"
            );
            assert_eq!(
                draft_store.delete(&valid.target).unwrap_err(),
                expected,
                "delete case {index}"
            );
            assert_eq!(
                fs::read(directory.state_path()).unwrap(),
                bytes,
                "case {index}"
            );
        }
    }

    #[test]
    fn duplicate_repository_and_commit_keys_are_rejected_before_flattening() {
        let sha = sha('a');
        let duplicate_repository =
            "{\"version\":1,\"repositories\":{\"1\":{\"commits\":{}},\"1\":{\"commits\":{}}}}"
                .to_owned();
        let duplicate_commit = format!(
            "{{\"version\":1,\"repositories\":{{\"1\":{{\"commits\":{{\"{sha}\":{{\"lines\":[]}},\"{sha}\":{{\"lines\":[]}}}}}}}}}}"
        );

        for json in [duplicate_repository, duplicate_commit] {
            assert!(serde_json::from_str::<serde_json::Value>(&json).is_ok());
            assert_eq!(
                decode(json.as_bytes()).unwrap_err(),
                DraftStateError::Malformed
            );
        }
    }

    #[test]
    fn oversized_file_is_malformed_and_left_untouched() {
        let directory = TestDirectory::new();
        fs::create_dir(directory.0.join("reviewbox")).unwrap();
        let file = File::create(directory.state_path()).unwrap();
        file.set_len(MAX_STATE_BYTES + 1).unwrap();
        drop(file);
        let draft = commit_draft(1, &sha('a'), "body");

        assert_eq!(
            store(&directory).load().unwrap_err(),
            DraftStateError::Malformed
        );
        assert_eq!(
            store(&directory).save(&draft).unwrap_err(),
            DraftStateError::Malformed
        );
        assert_eq!(
            store(&directory).delete(&draft.target).unwrap_err(),
            DraftStateError::Malformed
        );
        assert_eq!(
            fs::metadata(directory.state_path()).unwrap().len(),
            MAX_STATE_BYTES + 1
        );
    }

    #[test]
    fn injected_write_failure_preserves_bytes_and_removes_temporary_file() {
        let directory = TestDirectory::new();
        let first = commit_draft(3, &sha('1'), "first");
        let second = commit_draft(3, &sha('2'), "second");
        store(&directory).save(&first).unwrap();
        let before = fs::read(directory.state_path()).unwrap();

        let error = store(&directory)
            .failing_before_rename()
            .save(&second)
            .unwrap_err();

        assert!(matches!(error, DraftStateError::Write(_)));
        assert_eq!(fs::read(directory.state_path()).unwrap(), before);
        assert_eq!(
            fs::read_dir(directory.state_path().parent().unwrap())
                .unwrap()
                .count(),
            1
        );
    }

    #[test]
    fn xdg_home_empty_and_relative_resolution_is_exact() {
        let xdg = env::temp_dir().join("reviewbox-fictional-xdg");
        let home = env::temp_dir().join("reviewbox-fictional-home");
        let from_xdg = FileDraftStore::from_env(|name| match name {
            "XDG_DATA_HOME" => Some(xdg.clone().into_os_string()),
            "HOME" => Some(home.clone().into_os_string()),
            _ => None,
        })
        .unwrap();
        assert_eq!(from_xdg.path(), xdg.join("reviewbox").join(STATE_FILE_NAME));

        for ignored_xdg in [OsString::new(), OsString::from("relative/data")] {
            let fallback = FileDraftStore::from_env(|name| match name {
                "XDG_DATA_HOME" => Some(ignored_xdg.clone()),
                "HOME" => Some(home.clone().into_os_string()),
                _ => None,
            })
            .unwrap();
            assert_eq!(
                fallback.path(),
                home.join(".local/share/reviewbox").join(STATE_FILE_NAME)
            );
        }
        for lookup in [
            FileDraftStore::from_env(|_| None),
            FileDraftStore::from_env(|name| (name == "HOME").then(|| "relative".into())),
            FileDraftStore::at("relative/comment-drafts.json"),
        ] {
            assert_eq!(lookup.unwrap_err(), DraftStateError::Unavailable);
        }
    }

    #[cfg(unix)]
    #[test]
    fn atomic_write_creates_private_directory_and_file() {
        use std::os::unix::fs::PermissionsExt;

        let directory = TestDirectory::new();
        store(&directory)
            .save(&commit_draft(1, &sha('a'), "private"))
            .unwrap();
        let directory_mode = fs::metadata(directory.0.join("reviewbox"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        let file_mode = fs::metadata(directory.state_path())
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(directory_mode, 0o700);
        assert_eq!(file_mode, 0o600);
    }

    #[test]
    fn read_write_and_validation_errors_are_sanitized() {
        let directory = TestDirectory::new();
        let private_path = directory.0.join("fictional-secret-repository");
        fs::create_dir(&private_path).unwrap();
        let read_error = FileDraftStore::at(&private_path)
            .unwrap()
            .load()
            .unwrap_err();
        let write_error = store(&directory)
            .failing_before_rename()
            .save(&commit_draft(1, &sha('a'), "fictional private body"))
            .unwrap_err();
        let invalid_error = CommentDraft {
            target: CommentTarget::commit(1, sha('a')).unwrap(),
            body: "fictional private body".repeat(MAX_DRAFT_CHARACTERS),
        };
        let invalid_error = MemoryDraftStore::default()
            .save(&invalid_error)
            .unwrap_err();

        assert!(matches!(read_error, DraftStateError::Read(_)));
        assert!(matches!(write_error, DraftStateError::Write(_)));
        for error in [read_error, write_error, invalid_error] {
            let message = error.to_string();
            assert!(message.contains(STATE_FILE_NAME));
            assert!(!message.contains("fictional-secret-repository"));
            assert!(!message.contains("fictional private body"));
            assert!(!message.contains(&sha('a')));
        }
    }

    #[test]
    fn memory_store_updates_without_creating_files() {
        let directory = TestDirectory::new();
        let store = MemoryDraftStore::default();
        let first = commit_draft(1, &sha('a'), "first");
        let second = line_draft(1, &sha('a'), "src/lib.rs", 1, "second");
        store.save(&first).unwrap();
        store.save(&second).unwrap();
        assert_eq!(store.load().unwrap().len(), 2);
        assert_eq!(store.delete(&first.target).unwrap().len(), 1);
        assert_eq!(fs::read_dir(&directory.0).unwrap().count(), 0);
    }

    fn eligible_file(patch: PatchContent) -> FileChange {
        FileChange {
            path: "src/lib.rs".to_owned(),
            api_path_is_commentable: true,
            previous_path: None,
            status: FileStatus::Modified,
            additions: 0,
            deletions: 0,
            changes: 0,
            patch,
        }
    }

    #[test]
    fn line_eligibility_uses_raw_patch_positions_and_supported_rows_only() {
        let patch = parse_patch_text(
            "@@ -1,2 +1,2 @@\n context\n-deleted\n+added\n\\ No newline at end of file\n@@ -9 +9 @@\n later\nmetadata",
        );
        let file = eligible_file(patch.clone());
        assert!(line_target(&file, 0).is_none());
        for (row, expected) in [
            (1, true),
            (2, true),
            (3, true),
            (4, false),
            (5, false),
            (6, true),
            (7, false),
        ] {
            assert_eq!(line_target(&file, row).is_some(), expected, "row {row}");
        }
        assert_eq!(line_target(&file, 6).unwrap().position, 6);
        assert!(line_target(&file, 8).is_none());

        let retained = patch.lines().to_vec();
        let file_limit = eligible_file(PatchContent::Capped {
            lines: retained,
            omitted_lines: 10,
            omitted_bytes: 100,
            reason: PatchCapReason::FileLimit,
        });
        assert_eq!(line_target(&file_limit, 1).unwrap().position, 1);
        let commit_limit = eligible_file(PatchContent::Capped {
            lines: patch.lines().to_vec(),
            omitted_lines: 10,
            omitted_bytes: 100,
            reason: PatchCapReason::CommitBudget,
        });
        assert!(line_target(&commit_limit, 1).is_none());
    }

    #[test]
    fn line_eligibility_rejects_non_hunk_notice_and_changed_api_paths() {
        for patch in [
            parse_patch_text("+not in a hunk"),
            PatchContent::Empty,
            PatchContent::NoPatch,
            PatchContent::Unavailable,
        ] {
            assert!(line_target(&eligible_file(patch), 1).is_none());
        }
        for sanitized_path in ["src/four    spaces.rs", "src/bad�name.rs"] {
            let mut file = eligible_file(parse_patch_text("@@ -1 +1 @@\n+line"));
            file.path = sanitized_path.to_owned();
            file.api_path_is_commentable = false;
            assert!(line_target(&file, 1).is_none());
        }
    }
}
