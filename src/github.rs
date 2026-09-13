//! Read-only GitHub discovery and daily commit loading through `gh api`.
//!
//! The loader is deliberately independent of the terminal UI. Callers receive
//! incremental events synchronously and can run it on their own worker thread.

use std::collections::{BTreeMap, HashMap};
use std::ffi::{OsStr, OsString};
use std::fmt;
use std::io::{self, Read};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use chrono::{DateTime, SecondsFormat, Utc};
use serde::Deserialize;
use serde::de::DeserializeOwned;

use crate::day::DaySelection;
use crate::inbox::{
    ChildPane, Commit, CommitDetail, DiffLine, DiffLineKind, FileChange, FileStatus, GitHubAuthor,
    PatchCapReason, PatchContent, Repository, RepositoryIdentity,
};

const PAGE_SIZE: usize = 100;
const MAX_STDOUT_BYTES: usize = 16 * 1024 * 1024;
const MAX_STDERR_BYTES: usize = 8 * 1024;
pub const MAX_DETAIL_FILES: usize = 300;
pub const MAX_FILE_PATCH_BYTES: usize = 256 * 1024;
pub const MAX_FILE_PATCH_LINES: usize = 5_000;
pub const MAX_DIFF_LINE_CHARS: usize = 2_000;
pub const MAX_COMMIT_PATCH_BYTES: usize = 2 * 1024 * 1024;

/// A bounded byte buffer returned by a process runner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoundedBytes {
    pub bytes: Vec<u8>,
    pub truncated: bool,
}

/// The result of one direct process invocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessOutput {
    pub success: bool,
    pub exit_code: Option<i32>,
    pub stdout: BoundedBytes,
    pub stderr: BoundedBytes,
}

/// A process-start or process-transport failure, intentionally without raw OS
/// error text so it cannot accidentally become a user-facing secret leak.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessError {
    NotFound,
    Transport,
    Cancelled,
}

#[derive(Debug, Clone, Default)]
pub struct CancellationToken(Arc<AtomicBool>);

impl CancellationToken {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}

/// Injectable boundary used for every `gh` invocation.
pub trait ProcessRunner {
    fn run(
        &self,
        executable: &OsStr,
        arguments: &[OsString],
    ) -> Result<ProcessOutput, ProcessError>;

    fn run_cancellable(
        &self,
        executable: &OsStr,
        arguments: &[OsString],
        cancellation: &CancellationToken,
    ) -> Result<ProcessOutput, ProcessError> {
        if cancellation.is_cancelled() {
            Err(ProcessError::Cancelled)
        } else {
            self.run(executable, arguments)
        }
    }
}

/// Production runner. It invokes `gh` directly, never through a shell, and
/// drains both pipes concurrently to avoid subprocess deadlocks.
#[derive(Debug, Default, Clone, Copy)]
pub struct CommandRunner;

impl ProcessRunner for CommandRunner {
    fn run(
        &self,
        executable: &OsStr,
        arguments: &[OsString],
    ) -> Result<ProcessOutput, ProcessError> {
        self.run_cancellable(executable, arguments, &CancellationToken::default())
    }

    fn run_cancellable(
        &self,
        executable: &OsStr,
        arguments: &[OsString],
        cancellation: &CancellationToken,
    ) -> Result<ProcessOutput, ProcessError> {
        if cancellation.is_cancelled() {
            return Err(ProcessError::Cancelled);
        }
        let mut child = Command::new(executable)
            .args(arguments)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| {
                if error.kind() == io::ErrorKind::NotFound {
                    ProcessError::NotFound
                } else {
                    ProcessError::Transport
                }
            })?;

        let stdout = child.stdout.take().ok_or(ProcessError::Transport)?;
        let stderr = child.stderr.take().ok_or(ProcessError::Transport)?;
        let stdout_reader = thread::spawn(move || read_bounded(stdout, MAX_STDOUT_BYTES));
        let stderr_reader = thread::spawn(move || read_bounded(stderr, MAX_STDERR_BYTES));

        let status = loop {
            if cancellation.is_cancelled() {
                let _ = child.kill();
                let _ = child.wait();
                let _ = stdout_reader.join();
                let _ = stderr_reader.join();
                return Err(ProcessError::Cancelled);
            }
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) => thread::sleep(Duration::from_millis(10)),
                Err(_) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    let _ = stdout_reader.join();
                    let _ = stderr_reader.join();
                    return Err(ProcessError::Transport);
                }
            }
        };
        let stdout = stdout_reader
            .join()
            .map_err(|_| ProcessError::Transport)?
            .map_err(|_| ProcessError::Transport)?;
        let stderr = stderr_reader
            .join()
            .map_err(|_| ProcessError::Transport)?
            .map_err(|_| ProcessError::Transport)?;

        Ok(ProcessOutput {
            success: status.success(),
            exit_code: status.code(),
            stdout,
            stderr,
        })
    }
}

fn read_bounded(mut reader: impl Read, limit: usize) -> io::Result<BoundedBytes> {
    let mut bytes = Vec::with_capacity(limit.min(8192));
    let mut buffer = [0_u8; 8192];
    let mut truncated = false;
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        let remaining = limit.saturating_sub(bytes.len());
        let retained = remaining.min(read);
        bytes.extend_from_slice(&buffer[..retained]);
        truncated |= retained < read;
    }
    Ok(BoundedBytes { bytes, truncated })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureCategory {
    Authentication,
    MissingGh,
    PermissionOrNotFound,
    RateLimit,
    MalformedResponse,
    MalformedJson,
    Offline,
    Transport,
    Command,
    Api,
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureScope {
    Authentication,
    Discovery,
    Repository {
        repository_index: usize,
    },
    Branch {
        repository_index: usize,
        branch_index: usize,
    },
    CommitDetail,
}

/// Sanitized error information. It never retains response bodies, stderr,
/// repository names, branch names, commit SHAs, or subjects.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadFailure {
    pub category: FailureCategory,
    pub scope: FailureScope,
    pub http_status: Option<u16>,
}

impl fmt::Display for LoadFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let category = match self.category {
            FailureCategory::Authentication => "GitHub authentication failed",
            FailureCategory::MissingGh => "GitHub CLI is unavailable",
            FailureCategory::PermissionOrNotFound => {
                "GitHub resource is unavailable or access was denied"
            }
            FailureCategory::RateLimit => "GitHub API rate limit was reached",
            FailureCategory::MalformedResponse => "GitHub returned an unreadable response",
            FailureCategory::MalformedJson => "GitHub returned malformed JSON",
            FailureCategory::Offline => "GitHub appears to be offline",
            FailureCategory::Transport => "GitHub CLI transport failed",
            FailureCategory::Command => "GitHub CLI request failed",
            FailureCategory::Api => "GitHub API request failed",
            FailureCategory::Cancelled => "GitHub loading was cancelled",
        };
        let scope = match self.scope {
            FailureScope::Authentication => " while resolving the authenticated account".to_owned(),
            FailureScope::Discovery => " during repository discovery".to_owned(),
            FailureScope::Repository { repository_index } => {
                format!(
                    " while enumerating branches for repository {}",
                    repository_index + 1
                )
            }
            FailureScope::Branch {
                repository_index,
                branch_index,
            } => format!(
                " while loading branch {} of repository {}",
                branch_index + 1,
                repository_index + 1
            ),
            FailureScope::CommitDetail => " while loading commit details".to_owned(),
        };
        write!(formatter, "{category}{scope}")?;
        if let Some(status) = self.http_status {
            write!(formatter, " (HTTP {status})")?;
        }
        Ok(())
    }
}

impl std::error::Error for LoadFailure {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DetailFailure {
    ResponseTruncated,
    Load(LoadFailure),
}

impl fmt::Display for DetailFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ResponseTruncated => {
                formatter.write_str("GitHub commit response exceeded the local size limit")
            }
            Self::Load(failure) => failure.fmt(formatter),
        }
    }
}

impl std::error::Error for DetailFailure {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DetailState {
    NotRequested,
    Loading,
    Ready(Arc<CommitDetail>),
    Failed(DetailFailure),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepositoryCoverage {
    Complete,
    BranchEnumerationFailed,
    BranchesIncomplete { failed_branches: usize },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadedRepository {
    pub repository: Repository,
    pub branch_count: usize,
    pub coverage: RepositoryCoverage,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoadStatus {
    Complete,
    Incomplete,
    Fatal,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LoadProgress {
    pub repositories_discovered: usize,
    pub repositories_processed: usize,
    pub branches_discovered: usize,
    pub branches_processed: usize,
    pub commits_loaded: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadReport {
    pub status: LoadStatus,
    pub repositories: Vec<LoadedRepository>,
    pub failures: Vec<LoadFailure>,
    pub progress: LoadProgress,
}

impl LoadReport {
    pub fn no_owned_repositories(&self) -> bool {
        self.status == LoadStatus::Complete && self.repositories.is_empty()
    }

    pub fn is_complete_empty_day(&self) -> bool {
        self.status == LoadStatus::Complete
            && !self.repositories.is_empty()
            && self
                .repositories
                .iter()
                .all(|repository| repository.repository.commits.is_empty())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoadEvent {
    DiscoveryPage {
        page: usize,
        owned_repositories: usize,
    },
    RepositoriesDiscovered {
        total: usize,
    },
    RepositoryStarted {
        repository_index: usize,
        total: usize,
    },
    BranchPage {
        repository_index: usize,
        page: usize,
        branches: usize,
    },
    BranchStarted {
        repository_index: usize,
        branch_index: usize,
        total: usize,
    },
    CommitPage {
        repository_index: usize,
        branch_index: usize,
        page: usize,
        accepted_commits: usize,
    },
    RepositoryLoaded {
        repository_index: usize,
        repository: LoadedRepository,
    },
    RepositorySnapshot {
        repository_index: usize,
        repository: LoadedRepository,
    },
    Failure(LoadFailure),
    Finished {
        status: LoadStatus,
        progress: LoadProgress,
    },
}

pub struct GitHubLoader<R> {
    runner: R,
    executable: OsString,
}

impl<R: ProcessRunner> GitHubLoader<R> {
    pub fn new(runner: R) -> Self {
        Self {
            runner,
            executable: OsString::from("gh"),
        }
    }

    /// Load all owned repositories and the selected user's commits for the
    /// selected civil day. Discovery failures are fatal; repository and branch
    /// failures emit sanitized events, preserve partials, and continue.
    pub fn load(
        &self,
        selection: &DaySelection,
        mut emit: impl FnMut(LoadEvent),
    ) -> Result<LoadReport, LoadFailure> {
        self.load_cancellable(selection, &CancellationToken::default(), &mut emit)
    }

    pub fn load_cancellable(
        &self,
        selection: &DaySelection,
        cancellation: &CancellationToken,
        mut emit: impl FnMut(LoadEvent),
    ) -> Result<LoadReport, LoadFailure> {
        let mut progress = LoadProgress::default();
        let user: ApiUser =
            match self.get_json("/user", &[], FailureScope::Authentication, cancellation) {
                Ok(user) => user,
                Err(failure) => {
                    emit(LoadEvent::Failure(failure.clone()));
                    emit(LoadEvent::Finished {
                        status: LoadStatus::Fatal,
                        progress,
                    });
                    return Err(failure);
                }
            };

        let repositories = match self.discover_repositories(&user.login, cancellation, &mut emit) {
            Ok(repositories) => repositories,
            Err(failure) => {
                emit(LoadEvent::Failure(failure.clone()));
                emit(LoadEvent::Finished {
                    status: LoadStatus::Fatal,
                    progress,
                });
                return Err(failure);
            }
        };
        progress.repositories_discovered = repositories.len();
        emit(LoadEvent::RepositoriesDiscovered {
            total: repositories.len(),
        });

        let mut loaded = Vec::with_capacity(repositories.len());
        let mut failures = Vec::new();
        for (repository_index, repository) in repositories.iter().enumerate() {
            emit(LoadEvent::RepositoryStarted {
                repository_index,
                total: repositories.len(),
            });
            let branches =
                match self.load_branches(repository, repository_index, cancellation, &mut emit) {
                    Ok(branches) => branches,
                    Err(failure) => {
                        if failure.category == FailureCategory::Cancelled {
                            return Err(failure);
                        }
                        emit(LoadEvent::Failure(failure.clone()));
                        failures.push(failure);
                        progress.repositories_processed += 1;
                        let partial = LoadedRepository {
                            repository: Repository {
                                identity: repository.identity(),
                                commits: Vec::new(),
                            },
                            branch_count: 0,
                            coverage: RepositoryCoverage::BranchEnumerationFailed,
                        };
                        emit(LoadEvent::RepositoryLoaded {
                            repository_index,
                            repository: partial.clone(),
                        });
                        loaded.push(partial);
                        continue;
                    }
                };

            progress.branches_discovered += branches.len();
            let mut commits = BTreeMap::<String, Commit>::new();
            let mut failed_branches = 0;
            for (branch_index, branch) in branches.iter().enumerate() {
                emit(LoadEvent::BranchStarted {
                    repository_index,
                    branch_index,
                    total: branches.len(),
                });
                match self.load_commits(
                    repository,
                    branch,
                    &user.login,
                    selection,
                    repository_index,
                    branch_index,
                    cancellation,
                    &mut emit,
                ) {
                    Ok(branch_commits) => {
                        for commit in branch_commits {
                            commits.entry(commit.sha.clone()).or_insert(commit);
                        }
                    }
                    Err(failure) => {
                        if failure.category == FailureCategory::Cancelled {
                            return Err(failure);
                        }
                        failed_branches += 1;
                        emit(LoadEvent::Failure(failure.clone()));
                        failures.push(failure);
                    }
                }
                progress.branches_processed += 1;

                let mut snapshot_commits = commits.values().cloned().collect::<Vec<_>>();
                snapshot_commits.sort_by(|left, right| {
                    right
                        .authored_at
                        .cmp(&left.authored_at)
                        .then_with(|| left.sha.cmp(&right.sha))
                });
                emit(LoadEvent::RepositorySnapshot {
                    repository_index,
                    repository: LoadedRepository {
                        repository: Repository {
                            identity: repository.identity(),
                            commits: snapshot_commits,
                        },
                        branch_count: branches.len(),
                        coverage: if failed_branches == 0 {
                            RepositoryCoverage::Complete
                        } else {
                            RepositoryCoverage::BranchesIncomplete { failed_branches }
                        },
                    },
                });
            }

            let mut commits = commits.into_values().collect::<Vec<_>>();
            commits.sort_by(|left, right| {
                right
                    .authored_at
                    .cmp(&left.authored_at)
                    .then_with(|| left.sha.cmp(&right.sha))
            });
            progress.commits_loaded += commits.len();
            progress.repositories_processed += 1;
            let coverage = if failed_branches == 0 {
                RepositoryCoverage::Complete
            } else {
                RepositoryCoverage::BranchesIncomplete { failed_branches }
            };
            let partial = LoadedRepository {
                repository: Repository {
                    identity: repository.identity(),
                    commits,
                },
                branch_count: branches.len(),
                coverage,
            };
            emit(LoadEvent::RepositoryLoaded {
                repository_index,
                repository: partial.clone(),
            });
            loaded.push(partial);
        }

        let status = if failures.is_empty() {
            LoadStatus::Complete
        } else {
            LoadStatus::Incomplete
        };
        emit(LoadEvent::Finished { status, progress });
        Ok(LoadReport {
            status,
            repositories: loaded,
            failures,
            progress,
        })
    }

    fn discover_repositories(
        &self,
        login: &str,
        cancellation: &CancellationToken,
        emit: &mut impl FnMut(LoadEvent),
    ) -> Result<Vec<ApiRepository>, LoadFailure> {
        let mut page = 1;
        let mut owned = Vec::new();
        loop {
            let fields = page_fields(page);
            let repositories: Vec<ApiRepository> = self.get_json(
                "/user/repos",
                &[
                    ("affiliation", "owner".to_owned()),
                    ("per_page", PAGE_SIZE.to_string()),
                    ("page", fields),
                ],
                FailureScope::Discovery,
                cancellation,
            )?;
            let page_len = repositories.len();
            owned.extend(
                repositories
                    .into_iter()
                    .filter(|repository| repository.owner.login.eq_ignore_ascii_case(login)),
            );
            emit(LoadEvent::DiscoveryPage {
                page,
                owned_repositories: owned.len(),
            });
            if page_len < PAGE_SIZE {
                break;
            }
            page += 1;
        }
        owned.sort_by(|left, right| stable_name_cmp(&left.full_name(), &right.full_name()));
        Ok(owned)
    }

    fn load_branches(
        &self,
        repository: &ApiRepository,
        repository_index: usize,
        cancellation: &CancellationToken,
        emit: &mut impl FnMut(LoadEvent),
    ) -> Result<Vec<String>, LoadFailure> {
        let endpoint = format!(
            "/repos/{}/{}/branches",
            encode_path_segment(&repository.owner.login),
            encode_path_segment(&repository.name)
        );
        let mut page = 1;
        let mut branches = Vec::new();
        loop {
            let values: Vec<ApiBranch> = self.get_json(
                &endpoint,
                &[
                    ("per_page", PAGE_SIZE.to_string()),
                    ("page", page_fields(page)),
                ],
                FailureScope::Repository { repository_index },
                cancellation,
            )?;
            let page_len = values.len();
            branches.extend(values.into_iter().map(|branch| branch.name));
            emit(LoadEvent::BranchPage {
                repository_index,
                page,
                branches: branches.len(),
            });
            if page_len < PAGE_SIZE {
                break;
            }
            page += 1;
        }
        branches.sort_by(|left, right| stable_name_cmp(left, right));
        branches.dedup();
        Ok(branches)
    }

    #[allow(clippy::too_many_arguments)]
    fn load_commits(
        &self,
        repository: &ApiRepository,
        branch: &str,
        login: &str,
        selection: &DaySelection,
        repository_index: usize,
        branch_index: usize,
        cancellation: &CancellationToken,
        emit: &mut impl FnMut(LoadEvent),
    ) -> Result<Vec<Commit>, LoadFailure> {
        let endpoint = format!(
            "/repos/{}/{}/commits",
            encode_path_segment(&repository.owner.login),
            encode_path_segment(&repository.name)
        );
        let start = format_timestamp(selection.interval.start);
        let end = format_timestamp(selection.interval.end);
        let mut page = 1;
        let mut accepted = Vec::new();
        loop {
            let values: Vec<ApiCommit> = self.get_json(
                &endpoint,
                &[
                    ("sha", branch.to_owned()),
                    ("author", login.to_owned()),
                    ("since", start.clone()),
                    ("until", end.clone()),
                    ("per_page", PAGE_SIZE.to_string()),
                    ("page", page_fields(page)),
                ],
                FailureScope::Branch {
                    repository_index,
                    branch_index,
                },
                cancellation,
            )?;
            let page_len = values.len();
            for value in values {
                let Some(author) = value.author else {
                    continue;
                };
                if !author.login.eq_ignore_ascii_case(login)
                    || !selection.interval.contains(value.commit.author.date)
                {
                    continue;
                }
                accepted.push(Commit {
                    sha: value.sha,
                    subject: commit_subject(&value.commit.message),
                    author: GitHubAuthor {
                        login: author.login,
                    },
                    authored_at: value.commit.author.date,
                    files: ChildPane::Unavailable,
                });
            }
            emit(LoadEvent::CommitPage {
                repository_index,
                branch_index,
                page,
                accepted_commits: accepted.len(),
            });
            if page_len < PAGE_SIZE {
                break;
            }
            page += 1;
        }
        Ok(accepted)
    }

    /// Load one commit's changed files on demand. The SHA must be a complete
    /// lowercase hexadecimal Git object id, so invalid identities never reach
    /// the process boundary.
    pub fn load_commit_detail(
        &self,
        repository: &RepositoryIdentity,
        sha: &str,
        cancellation: &CancellationToken,
    ) -> Result<CommitDetail, DetailFailure> {
        if !valid_full_sha(sha) {
            return Err(DetailFailure::Load(LoadFailure {
                category: FailureCategory::MalformedResponse,
                scope: FailureScope::CommitDetail,
                http_status: None,
            }));
        }

        let endpoint = format!(
            "/repos/{}/{}/commits/{}",
            encode_path_segment(&repository.owner),
            encode_path_segment(&repository.name),
            encode_path_segment(sha)
        );
        let output = self
            .run_get(&endpoint, &[], FailureScope::CommitDetail, cancellation)
            .map_err(DetailFailure::Load)?;
        if output.stdout.truncated {
            return Err(DetailFailure::ResponseTruncated);
        }
        let response: JsonResponse<ApiCommitDetail> = self
            .decode_json_output(output, FailureScope::CommitDetail)
            .map_err(DetailFailure::Load)?;
        Ok(commit_detail_from_api(response.value, &response.headers))
    }

    fn get_json<T: DeserializeOwned>(
        &self,
        endpoint: &str,
        fields: &[(&str, String)],
        scope: FailureScope,
        cancellation: &CancellationToken,
    ) -> Result<T, LoadFailure> {
        let output = self.run_get(endpoint, fields, scope, cancellation)?;
        if output.stdout.truncated {
            return Err(LoadFailure {
                category: FailureCategory::MalformedResponse,
                scope,
                http_status: None,
            });
        }
        self.decode_json_output(output, scope)
            .map(|response| response.value)
    }

    fn run_get(
        &self,
        endpoint: &str,
        fields: &[(&str, String)],
        scope: FailureScope,
        cancellation: &CancellationToken,
    ) -> Result<ProcessOutput, LoadFailure> {
        let mut arguments = vec![
            OsString::from("api"),
            OsString::from("--method"),
            OsString::from("GET"),
            OsString::from("--include"),
            OsString::from(endpoint),
        ];
        for (name, value) in fields {
            arguments.push(OsString::from("--raw-field"));
            arguments.push(OsString::from(format!("{name}={value}")));
        }

        self.runner
            .run_cancellable(&self.executable, &arguments, cancellation)
            .map_err(|error| LoadFailure {
                category: match error {
                    ProcessError::NotFound => FailureCategory::MissingGh,
                    ProcessError::Transport => FailureCategory::Transport,
                    ProcessError::Cancelled => FailureCategory::Cancelled,
                },
                scope,
                http_status: None,
            })
    }

    fn decode_json_output<T: DeserializeOwned>(
        &self,
        output: ProcessOutput,
        scope: FailureScope,
    ) -> Result<JsonResponse<T>, LoadFailure> {
        let envelope = parse_http_envelope(&output.stdout.bytes);
        if !output.success {
            let status = envelope
                .as_ref()
                .ok()
                .map(|value| value.status)
                .or_else(|| status_from_stderr(&output.stderr.bytes));
            let headers = envelope.as_ref().ok().map(|value| &value.headers);
            let category = if status.is_none()
                && scope == FailureScope::Authentication
                && stderr_indicates_missing_authentication(&output.stderr.bytes)
            {
                FailureCategory::Authentication
            } else if status.is_none()
                && scope == FailureScope::CommitDetail
                && stderr_indicates_offline(&output.stderr.bytes)
            {
                FailureCategory::Offline
            } else {
                classify_failure(status, headers)
            };
            return Err(LoadFailure {
                category,
                scope,
                http_status: status,
            });
        }

        let envelope = envelope.map_err(|()| LoadFailure {
            category: FailureCategory::MalformedResponse,
            scope,
            http_status: None,
        })?;
        if !(200..300).contains(&envelope.status) {
            return Err(LoadFailure {
                category: classify_failure(Some(envelope.status), Some(&envelope.headers)),
                scope,
                http_status: Some(envelope.status),
            });
        }
        let value = serde_json::from_slice(envelope.body).map_err(|_| LoadFailure {
            category: FailureCategory::MalformedJson,
            scope,
            http_status: Some(envelope.status),
        })?;
        Ok(JsonResponse {
            value,
            headers: envelope.headers,
        })
    }
}

struct JsonResponse<T> {
    value: T,
    headers: HashMap<String, String>,
}

fn valid_full_sha(sha: &str) -> bool {
    matches!(sha.len(), 40 | 64)
        && sha
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

fn commit_detail_from_api(
    detail: ApiCommitDetail,
    headers: &HashMap<String, String>,
) -> CommitDetail {
    let total_files = detail.files.len();
    let omitted_files = total_files.saturating_sub(MAX_DETAIL_FILES);
    let mut retained_bytes = 0_usize;
    let mut commit_budget_exhausted = false;
    let files = detail
        .files
        .into_iter()
        .take(MAX_DETAIL_FILES)
        .map(|file| {
            let patch = match file.patch {
                Some(patch) if patch.is_empty() => PatchContent::Empty,
                Some(patch) if commit_budget_exhausted => commit_budget_cap(&patch),
                Some(patch) => {
                    let parsed = parse_patch_text(&patch);
                    let patch_bytes = retained_patch_bytes(&parsed);
                    if retained_bytes.saturating_add(patch_bytes) > MAX_COMMIT_PATCH_BYTES {
                        commit_budget_exhausted = true;
                        commit_budget_cap(&patch)
                    } else {
                        retained_bytes += patch_bytes;
                        parsed
                    }
                }
                None if file.changes == 0 => PatchContent::Empty,
                None => PatchContent::Unavailable,
            };
            FileChange {
                path: sanitize_api_text(&file.filename),
                previous_path: file.previous_filename.as_deref().map(sanitize_api_text),
                status: FileStatus::from_api(&file.status),
                additions: file.additions,
                deletions: file.deletions,
                changes: file.changes,
                patch,
            }
        })
        .collect();
    CommitDetail {
        files,
        omitted_files,
        more_files_available: omitted_files > 0
            || headers
                .get("link")
                .is_some_and(|value| link_has_next(value)),
    }
}

fn commit_budget_cap(patch: &str) -> PatchContent {
    PatchContent::Capped {
        lines: Vec::new(),
        omitted_lines: patch_line_count(patch),
        omitted_bytes: patch.len(),
        reason: PatchCapReason::CommitBudget,
    }
}

fn retained_patch_bytes(patch: &PatchContent) -> usize {
    patch.lines().iter().map(|line| line.text.len()).sum()
}

fn patch_line_count(patch: &str) -> usize {
    if patch.is_empty() {
        0
    } else {
        patch.split_terminator('\n').count()
    }
}

fn link_has_next(value: &str) -> bool {
    value
        .split(',')
        .any(|link| link.split(';').any(|part| part.trim() == "rel=\"next\""))
}

impl FileStatus {
    fn from_api(status: &str) -> Self {
        match status {
            "added" => Self::Added,
            "modified" => Self::Modified,
            "removed" => Self::Removed,
            "renamed" => Self::Renamed,
            "copied" => Self::Copied,
            "changed" => Self::Changed,
            "unchanged" => Self::Unchanged,
            _ => Self::Unknown,
        }
    }
}

/// Convert a GitHub patch into bounded, numbered, terminal-safe logical lines.
/// The demo fixture uses this same parser as live commit details.
pub(crate) fn parse_patch_text(patch: &str) -> PatchContent {
    if patch.is_empty() {
        return PatchContent::Empty;
    }

    let raw_lines = patch.split_terminator('\n').collect::<Vec<_>>();
    let mut lines = Vec::with_capacity(raw_lines.len().min(MAX_FILE_PATCH_LINES));
    let mut retained_bytes = 0_usize;
    let mut omitted_lines = 0_usize;
    let mut omitted_bytes = 0_usize;
    let mut capped = false;
    let mut old_line = None;
    let mut new_line = None;

    for (index, raw_line) in raw_lines.iter().enumerate() {
        if lines.len() >= MAX_FILE_PATCH_LINES {
            capped = true;
            omitted_lines += raw_lines.len() - index;
            omitted_bytes += raw_lines[index..]
                .iter()
                .map(|line| line.len())
                .sum::<usize>();
            break;
        }

        let sanitized = sanitize_api_text(raw_line);
        let (text, line_omitted_bytes) = cap_line_text(&sanitized);
        if retained_bytes.saturating_add(text.len()) > MAX_FILE_PATCH_BYTES {
            capped = true;
            omitted_lines += raw_lines.len() - index;
            omitted_bytes += raw_lines[index..]
                .iter()
                .map(|line| line.len())
                .sum::<usize>();
            break;
        }
        capped |= line_omitted_bytes > 0;
        omitted_bytes += line_omitted_bytes;
        retained_bytes += text.len();

        let (kind, numbered_old, numbered_new) = if let Some((old, new)) = parse_hunk_header(&text)
        {
            old_line = Some(old);
            new_line = Some(new);
            (DiffLineKind::Hunk, None, None)
        } else if text.starts_with("@@") {
            old_line = None;
            new_line = None;
            (DiffLineKind::Other, None, None)
        } else if text.starts_with("\\ No newline") {
            (DiffLineKind::NoNewline, None, None)
        } else if text.starts_with(' ') {
            let values = (old_line, new_line);
            old_line = old_line.and_then(|line| line.checked_add(1));
            new_line = new_line.and_then(|line| line.checked_add(1));
            (DiffLineKind::Context, values.0, values.1)
        } else if text.starts_with('+') {
            let value = new_line;
            new_line = new_line.and_then(|line| line.checked_add(1));
            (DiffLineKind::Addition, None, value)
        } else if text.starts_with('-') {
            let value = old_line;
            old_line = old_line.and_then(|line| line.checked_add(1));
            (DiffLineKind::Deletion, value, None)
        } else {
            (DiffLineKind::Other, None, None)
        };
        lines.push(DiffLine {
            kind,
            old_line: numbered_old,
            new_line: numbered_new,
            text,
        });
    }

    if capped {
        PatchContent::Capped {
            lines,
            omitted_lines,
            omitted_bytes,
            reason: PatchCapReason::FileLimit,
        }
    } else {
        PatchContent::Text { lines }
    }
}

fn cap_line_text(value: &str) -> (String, usize) {
    if value.chars().count() <= MAX_DIFF_LINE_CHARS {
        return (value.to_owned(), 0);
    }
    let prefix = value
        .chars()
        .take(MAX_DIFF_LINE_CHARS.saturating_sub(1))
        .collect::<String>();
    let omitted_bytes = value.len().saturating_sub(prefix.len());
    (format!("{prefix}…"), omitted_bytes)
}

fn sanitize_api_text(value: &str) -> String {
    let value = value.strip_suffix('\r').unwrap_or(value);
    let mut sanitized = String::with_capacity(value.len());
    for character in value.chars() {
        if character == '\t' {
            sanitized.push_str("    ");
        } else if character.is_control() {
            sanitized.push('\u{fffd}');
        } else {
            sanitized.push(character);
        }
    }
    sanitized
}

fn parse_hunk_header(line: &str) -> Option<(u32, u32)> {
    let remainder = line.strip_prefix("@@ -")?;
    let (old, remainder) = remainder.split_once(' ')?;
    let remainder = remainder.strip_prefix('+')?;
    let (new, suffix) = remainder.split_once(' ')?;
    if !suffix.starts_with("@@") {
        return None;
    }
    Some((parse_range_start(old)?, parse_range_start(new)?))
}

fn parse_range_start(value: &str) -> Option<u32> {
    value.split(',').next()?.parse().ok()
}

fn page_fields(page: usize) -> String {
    page.to_string()
}

fn format_timestamp(timestamp: DateTime<Utc>) -> String {
    timestamp.to_rfc3339_opts(SecondsFormat::Secs, true)
}

fn commit_subject(message: &str) -> String {
    message
        .lines()
        .next()
        .unwrap_or_default()
        .trim_end()
        .to_owned()
}

fn stable_name_cmp(left: &str, right: &str) -> std::cmp::Ordering {
    left.to_lowercase()
        .cmp(&right.to_lowercase())
        .then_with(|| left.cmp(right))
}

fn encode_path_segment(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            encoded.push(char::from(byte));
        } else {
            use std::fmt::Write as _;
            write!(encoded, "%{byte:02X}").expect("writing to a String cannot fail");
        }
    }
    encoded
}

struct HttpEnvelope<'a> {
    status: u16,
    headers: HashMap<String, String>,
    body: &'a [u8],
}

fn parse_http_envelope(bytes: &[u8]) -> Result<HttpEnvelope<'_>, ()> {
    // Version managers and other executable launchers can emit a bounded
    // informational line before handing control to `gh`. The response remains
    // unambiguous because an included HTTP envelope starts at a line boundary.
    let bytes = http_envelope_start(bytes).ok_or(())?;
    let (header, body) = split_header(bytes).ok_or(())?;
    let header = std::str::from_utf8(header).map_err(|_| ())?;
    let mut lines = header.lines();
    let status_line = lines.next().ok_or(())?.trim_end_matches('\r');
    if !status_line.starts_with("HTTP/") {
        return Err(());
    }
    let status = status_line
        .split_ascii_whitespace()
        .nth(1)
        .and_then(|value| value.parse::<u16>().ok())
        .ok_or(())?;
    let mut headers = HashMap::new();
    for line in lines {
        let line = line.trim_end_matches('\r');
        let (name, value) = line.split_once(':').ok_or(())?;
        headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_owned());
    }
    Ok(HttpEnvelope {
        status,
        headers,
        body,
    })
}

fn http_envelope_start(bytes: &[u8]) -> Option<&[u8]> {
    if bytes.starts_with(b"HTTP/") {
        return Some(bytes);
    }
    bytes
        .windows(b"\nHTTP/".len())
        .position(|window| window == b"\nHTTP/")
        .map(|index| &bytes[index + 1..])
}

fn split_header(bytes: &[u8]) -> Option<(&[u8], &[u8])> {
    if let Some(index) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
        return Some((&bytes[..index], &bytes[index + 4..]));
    }
    bytes
        .windows(2)
        .position(|window| window == b"\n\n")
        .map(|index| (&bytes[..index], &bytes[index + 2..]))
}

fn status_from_stderr(stderr: &[u8]) -> Option<u16> {
    let stderr = String::from_utf8_lossy(stderr);
    let marker = "HTTP ";
    let start = stderr.rfind(marker)? + marker.len();
    stderr.get(start..start + 3)?.parse().ok()
}

fn stderr_indicates_missing_authentication(stderr: &[u8]) -> bool {
    let stderr = String::from_utf8_lossy(stderr).to_ascii_lowercase();
    [
        "gh auth login",
        "not logged in",
        "not authenticated",
        "authentication required",
    ]
    .iter()
    .any(|marker| stderr.contains(marker))
}

fn stderr_indicates_offline(stderr: &[u8]) -> bool {
    let stderr = String::from_utf8_lossy(stderr).to_ascii_lowercase();
    [
        "error connecting to",
        "dial tcp",
        "no such host",
        "could not resolve",
        "network is unreachable",
    ]
    .iter()
    .any(|marker| stderr.contains(marker))
}

fn classify_failure(
    status: Option<u16>,
    headers: Option<&HashMap<String, String>>,
) -> FailureCategory {
    let rate_limited = headers.is_some_and(|headers| {
        headers
            .get("x-ratelimit-remaining")
            .is_some_and(|value| value == "0")
            || headers.contains_key("retry-after")
    });
    match status {
        Some(401) => FailureCategory::Authentication,
        Some(429) => FailureCategory::RateLimit,
        Some(403) if rate_limited => FailureCategory::RateLimit,
        Some(403 | 404) => FailureCategory::PermissionOrNotFound,
        Some(_) => FailureCategory::Api,
        None => FailureCategory::Command,
    }
}

#[derive(Debug, Deserialize)]
struct ApiUser {
    login: String,
}

#[derive(Debug, Deserialize)]
struct ApiRepository {
    id: u64,
    name: String,
    owner: ApiUser,
}

impl ApiRepository {
    fn full_name(&self) -> String {
        format!("{}/{}", self.owner.login, self.name)
    }

    fn identity(&self) -> RepositoryIdentity {
        RepositoryIdentity {
            id: self.id,
            owner: self.owner.login.clone(),
            name: self.name.clone(),
        }
    }
}

#[derive(Debug, Deserialize)]
struct ApiBranch {
    name: String,
}

#[derive(Debug, Deserialize)]
struct ApiCommit {
    sha: String,
    author: Option<ApiUser>,
    commit: ApiCommitData,
}

#[derive(Debug, Deserialize)]
struct ApiCommitData {
    author: ApiCommitAuthor,
    message: String,
}

#[derive(Debug, Deserialize)]
struct ApiCommitAuthor {
    date: DateTime<Utc>,
}

#[derive(Debug, Deserialize)]
struct ApiCommitDetail {
    files: Vec<ApiFileChange>,
}

#[derive(Debug, Deserialize)]
struct ApiFileChange {
    filename: String,
    previous_filename: Option<String>,
    status: String,
    additions: u64,
    deletions: u64,
    changes: u64,
    patch: Option<String>,
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::Mutex;

    use chrono::NaiveDate;
    use serde_json::{Value, json};

    use super::*;
    use crate::day::{TimezoneSource, parse_timezone, select_day};

    #[derive(Clone)]
    enum ScriptResult {
        Output(ProcessOutput),
        Error(ProcessError),
    }

    #[derive(Clone)]
    struct Step {
        endpoint: String,
        fields: Vec<String>,
        result: ScriptResult,
    }

    struct ScriptedRunner {
        steps: Mutex<VecDeque<Step>>,
        calls: Mutex<Vec<Vec<OsString>>>,
    }

    impl ScriptedRunner {
        fn new(steps: Vec<Step>) -> Self {
            Self {
                steps: Mutex::new(steps.into()),
                calls: Mutex::new(Vec::new()),
            }
        }

        fn assert_finished(&self) {
            assert!(self.steps.lock().unwrap().is_empty(), "unused script steps");
        }

        fn calls(&self) -> Vec<Vec<OsString>> {
            self.calls.lock().unwrap().clone()
        }
    }

    impl ProcessRunner for ScriptedRunner {
        fn run(
            &self,
            executable: &OsStr,
            arguments: &[OsString],
        ) -> Result<ProcessOutput, ProcessError> {
            assert_eq!(executable, OsStr::new("gh"));
            assert_eq!(arguments.first(), Some(&OsString::from("api")));
            assert_eq!(arguments.get(1), Some(&OsString::from("--method")));
            assert_eq!(arguments.get(2), Some(&OsString::from("GET")));
            assert_eq!(arguments.get(3), Some(&OsString::from("--include")));
            assert_eq!(arguments.len() % 2, 1, "fields must be argv pairs");
            for index in (5..arguments.len()).step_by(2) {
                assert_eq!(arguments[index], OsString::from("--raw-field"));
            }
            let step = self
                .steps
                .lock()
                .unwrap()
                .pop_front()
                .expect("unexpected gh invocation");
            assert_eq!(arguments[4], OsString::from(step.endpoint));
            assert_eq!(
                (6..arguments.len())
                    .step_by(2)
                    .map(|index| arguments[index].to_string_lossy().into_owned())
                    .collect::<Vec<_>>(),
                step.fields
            );
            self.calls.lock().unwrap().push(arguments.to_vec());
            match step.result {
                ScriptResult::Output(output) => Ok(output),
                ScriptResult::Error(error) => Err(error),
            }
        }
    }

    fn selection() -> DaySelection {
        select_day(
            NaiveDate::from_ymd_opt(2024, 1, 15).unwrap(),
            parse_timezone("Europe/Warsaw").unwrap(),
            TimezoneSource::Explicit,
        )
        .unwrap()
    }

    fn fields(values: &[(&str, &str)]) -> Vec<String> {
        values
            .iter()
            .map(|(name, value)| format!("{name}={value}"))
            .collect()
    }

    fn json_step(endpoint: impl Into<String>, fields: Vec<String>, body: Value) -> Step {
        Step {
            endpoint: endpoint.into(),
            fields,
            result: ScriptResult::Output(http_output(200, &[], body.to_string().as_bytes(), true)),
        }
    }

    fn failure_step(
        endpoint: impl Into<String>,
        fields: Vec<String>,
        status: u16,
        headers: &[(&str, &str)],
        body: &[u8],
    ) -> Step {
        Step {
            endpoint: endpoint.into(),
            fields,
            result: ScriptResult::Output(http_output(status, headers, body, false)),
        }
    }

    fn runner_error_step(
        endpoint: impl Into<String>,
        fields: Vec<String>,
        error: ProcessError,
    ) -> Step {
        Step {
            endpoint: endpoint.into(),
            fields,
            result: ScriptResult::Error(error),
        }
    }

    fn http_output(
        status: u16,
        headers: &[(&str, &str)],
        body: &[u8],
        success: bool,
    ) -> ProcessOutput {
        let mut stdout = format!("HTTP/2.0 {status} status\r\n").into_bytes();
        for (name, value) in headers {
            stdout.extend_from_slice(format!("{name}: {value}\r\n").as_bytes());
        }
        stdout.extend_from_slice(b"\r\n");
        stdout.extend_from_slice(body);
        ProcessOutput {
            success,
            exit_code: Some(if success { 0 } else { 1 }),
            stdout: BoundedBytes {
                bytes: stdout,
                truncated: false,
            },
            stderr: BoundedBytes {
                bytes: if success {
                    Vec::new()
                } else {
                    format!("private failure payload (HTTP {status})").into_bytes()
                },
                truncated: false,
            },
        }
    }

    fn user_step() -> Step {
        json_step("/user", vec![], json!({"login": "Octo"}))
    }

    fn repos_step(repositories: Value) -> Step {
        json_step(
            "/user/repos",
            fields(&[("affiliation", "owner"), ("per_page", "100"), ("page", "1")]),
            repositories,
        )
    }

    fn branch_fields(page: usize) -> Vec<String> {
        fields(&[("per_page", "100"), ("page", &page.to_string())])
    }

    fn commit_fields(branch: &str, page: usize) -> Vec<String> {
        fields(&[
            ("sha", branch),
            ("author", "Octo"),
            ("since", "2024-01-14T23:00:00Z"),
            ("until", "2024-01-15T23:00:00Z"),
            ("per_page", "100"),
            ("page", &page.to_string()),
        ])
    }

    fn commit(sha: &str, login: Option<&str>, date: &str, message: &str) -> Value {
        json!({
            "sha": sha,
            "author": login.map(|login| json!({"login": login})),
            "commit": {"author": {"date": date}, "message": message}
        })
    }

    #[test]
    fn paginates_discovery_filters_ownership_and_encodes_unusual_repository_names() {
        let mut first_page = (0..PAGE_SIZE)
            .map(|index| json!({"id": index, "name": format!("foreign-{index:03}"), "owner": {"login": "Else"}}))
            .collect::<Vec<_>>();
        first_page.reverse();
        let steps = vec![
            user_step(),
            json_step(
                "/user/repos",
                fields(&[("affiliation", "owner"), ("per_page", "100"), ("page", "1")]),
                Value::Array(first_page),
            ),
            json_step(
                "/user/repos",
                fields(&[("affiliation", "owner"), ("per_page", "100"), ("page", "2")]),
                json!([
                    {"id": 202, "name": "z-last", "owner": {"login": "octo"}},
                    {"id": 101, "name": "odd repo?#%", "owner": {"login": "OCTO"}}
                ]),
            ),
            json_step(
                "/repos/OCTO/odd%20repo%3F%23%25/branches",
                branch_fields(1),
                json!([]),
            ),
            json_step("/repos/octo/z-last/branches", branch_fields(1), json!([])),
        ];
        let runner = ScriptedRunner::new(steps);
        let mut events = Vec::new();
        let report = GitHubLoader::new(&runner)
            .load(&selection(), |event| events.push(event))
            .unwrap();

        assert_eq!(report.status, LoadStatus::Complete);
        assert_eq!(
            report
                .repositories
                .iter()
                .map(|value| value.repository.display_name())
                .collect::<Vec<_>>(),
            ["OCTO/odd repo?#%".to_owned(), "octo/z-last".to_owned()]
        );
        assert_eq!(report.repositories[0].repository.identity.id, 101);
        assert_eq!(report.repositories[0].repository.identity.owner, "OCTO");
        assert_eq!(
            report.repositories[0].repository.identity.name,
            "odd repo?#%"
        );
        assert!(report.repositories.iter().all(|value| {
            value.branch_count == 0 && value.coverage == RepositoryCoverage::Complete
        }));
        assert!(events.contains(&LoadEvent::DiscoveryPage {
            page: 2,
            owned_repositories: 2
        }));
        assert!(report.is_complete_empty_day());
        runner.assert_finished();
    }

    #[test]
    fn paginates_branches_and_commits_with_safe_fields_and_deterministic_results() {
        let repository = json!([{"id": 301, "name": "daily", "owner": {"login": "Octo"}}]);
        let mut first_branch_page = (0..PAGE_SIZE)
            .rev()
            .map(|index| json!({"name": format!("branch-{index:03}")}))
            .collect::<Vec<_>>();
        let unusual_branch = "feature/what&why=now";
        let mut steps = vec![
            user_step(),
            repos_step(repository),
            json_step(
                "/repos/Octo/daily/branches",
                branch_fields(1),
                Value::Array(std::mem::take(&mut first_branch_page)),
            ),
            json_step(
                "/repos/Octo/daily/branches",
                branch_fields(2),
                json!([{"name": unusual_branch}]),
            ),
        ];
        let repeated = commit(
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            Some("oCtO"),
            "2024-01-15T10:00:00Z",
            "Shared commit\n\nbody",
        );
        let first_commit_page = vec![repeated; PAGE_SIZE];
        steps.push(json_step(
            "/repos/Octo/daily/commits",
            commit_fields("branch-000", 1),
            Value::Array(first_commit_page),
        ));
        steps.push(json_step(
            "/repos/Octo/daily/commits",
            commit_fields("branch-000", 2),
            json!([
                commit(
                    "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
                    Some("Octo"),
                    "2024-01-15T20:00:00Z",
                    "Newest"
                ),
                commit(
                    "cccccccccccccccccccccccccccccccccccccccc",
                    Some("someone-else"),
                    "2024-01-15T12:00:00Z",
                    "Wrong user"
                ),
                commit(
                    "dddddddddddddddddddddddddddddddddddddddd",
                    None,
                    "2024-01-15T12:00:00Z",
                    "Unlinked"
                )
            ]),
        ));
        for index in 1..PAGE_SIZE {
            steps.push(json_step(
                "/repos/Octo/daily/commits",
                commit_fields(&format!("branch-{index:03}"), 1),
                json!([]),
            ));
        }
        steps.push(json_step(
            "/repos/Octo/daily/commits",
            commit_fields(unusual_branch, 1),
            json!([
                commit(
                    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                    Some("Octo"),
                    "2024-01-15T10:00:00Z",
                    "Shared commit"
                ),
                commit(
                    "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee",
                    Some("Octo"),
                    "2024-01-14T23:00:00Z",
                    "At start"
                ),
                commit(
                    "ffffffffffffffffffffffffffffffffffffffff",
                    Some("Octo"),
                    "2024-01-15T23:00:00Z",
                    "At excluded end"
                )
            ]),
        ));

        let runner = ScriptedRunner::new(steps);
        let mut events = Vec::new();
        let report = GitHubLoader::new(&runner)
            .load(&selection(), |event| events.push(event))
            .unwrap();
        let loaded = &report.repositories[0];

        assert_eq!(loaded.branch_count, 101);
        assert_eq!(loaded.coverage, RepositoryCoverage::Complete);
        assert_eq!(
            loaded
                .repository
                .commits
                .iter()
                .map(|commit| (commit.sha.as_str(), commit.subject.as_str()))
                .collect::<Vec<_>>(),
            [
                ("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb", "Newest"),
                ("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", "Shared commit"),
                ("eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee", "At start")
            ]
        );
        assert_eq!(report.progress.branches_discovered, 101);
        assert_eq!(report.progress.branches_processed, 101);
        assert_eq!(report.progress.commits_loaded, 3);
        assert!(
            events
                .iter()
                .any(|event| matches!(event, LoadEvent::CommitPage { page: 2, .. }))
        );
        assert!(runner.calls().iter().all(|arguments| {
            arguments.get(2).and_then(|value| value.to_str()) == Some("GET")
                && !arguments.iter().any(|argument| {
                    matches!(
                        argument.to_str(),
                        Some("POST" | "PATCH" | "PUT" | "DELETE" | "auth" | "token")
                    )
                })
        }));
        runner.assert_finished();
    }

    #[test]
    fn applies_exact_half_open_boundary_and_authenticated_author_checks() {
        let steps = vec![
            user_step(),
            repos_step(json!([{"id": 401, "name": "boundaries", "owner": {"login": "Octo"}}])),
            json_step(
                "/repos/Octo/boundaries/branches",
                branch_fields(1),
                json!([{"name": "main"}]),
            ),
            json_step(
                "/repos/Octo/boundaries/commits",
                commit_fields("main", 1),
                json!([
                    commit("1", Some("Octo"), "2024-01-14T22:59:59Z", "Before"),
                    commit("2", Some("Octo"), "2024-01-14T23:00:00Z", "Start"),
                    commit("3", Some("octo"), "2024-01-15T22:59:59Z", "End minus one"),
                    commit("4", Some("Octo"), "2024-01-15T23:00:00Z", "End"),
                    commit("5", Some("Other"), "2024-01-15T12:00:00Z", "Other"),
                    commit("6", None, "2024-01-15T12:00:00Z", "Unlinked")
                ]),
            ),
        ];
        let runner = ScriptedRunner::new(steps);
        let report = GitHubLoader::new(&runner)
            .load(&selection(), |_| {})
            .unwrap();
        assert_eq!(
            report.repositories[0]
                .repository
                .commits
                .iter()
                .map(|commit| commit.sha.as_str())
                .collect::<Vec<_>>(),
            ["3", "2"]
        );
        runner.assert_finished();
    }

    #[test]
    fn represents_branchless_and_empty_repositories_without_claiming_failure() {
        let steps = vec![
            user_step(),
            repos_step(json!([
                {"id": 501, "name": "branchless", "owner": {"login": "Octo"}},
                {"id": 502, "name": "empty-day", "owner": {"login": "Octo"}}
            ])),
            json_step(
                "/repos/Octo/branchless/branches",
                branch_fields(1),
                json!([]),
            ),
            json_step(
                "/repos/Octo/empty-day/branches",
                branch_fields(1),
                json!([{"name": "main"}]),
            ),
            json_step(
                "/repos/Octo/empty-day/commits",
                commit_fields("main", 1),
                json!([]),
            ),
        ];
        let runner = ScriptedRunner::new(steps);
        let report = GitHubLoader::new(&runner)
            .load(&selection(), |_| {})
            .unwrap();

        assert_eq!(report.status, LoadStatus::Complete);
        assert!(report.failures.is_empty());
        assert_eq!(report.repositories[0].branch_count, 0);
        assert_eq!(report.repositories[1].branch_count, 1);
        assert!(report.is_complete_empty_day());
        runner.assert_finished();
    }

    #[test]
    fn returns_an_explicit_no_owned_repositories_state() {
        let runner = ScriptedRunner::new(vec![user_step(), repos_step(json!([]))]);
        let report = GitHubLoader::new(&runner)
            .load(&selection(), |_| {})
            .unwrap();

        assert!(report.no_owned_repositories());
        assert!(!report.is_complete_empty_day());
        runner.assert_finished();
    }

    #[test]
    fn continues_after_repository_and_branch_failures_and_preserves_partials() {
        let secret = br#"{"message":"secret repository payload"}"#;
        let steps = vec![
            user_step(),
            repos_step(json!([
                {"id": 601, "name": "alpha-private", "owner": {"login": "Octo"}},
                {"id": 602, "name": "beta-private", "owner": {"login": "Octo"}}
            ])),
            failure_step(
                "/repos/Octo/alpha-private/branches",
                branch_fields(1),
                403,
                &[("x-ratelimit-remaining", "27")],
                secret,
            ),
            json_step(
                "/repos/Octo/beta-private/branches",
                branch_fields(1),
                json!([{"name": "broken"}, {"name": "working"}]),
            ),
            failure_step(
                "/repos/Octo/beta-private/commits",
                commit_fields("broken", 1),
                404,
                &[],
                secret,
            ),
            json_step(
                "/repos/Octo/beta-private/commits",
                commit_fields("working", 1),
                json!([commit(
                    "kept",
                    Some("Octo"),
                    "2024-01-15T12:00:00Z",
                    "Kept partial"
                )]),
            ),
        ];
        let runner = ScriptedRunner::new(steps);
        let mut events = Vec::new();
        let report = GitHubLoader::new(&runner)
            .load(&selection(), |event| events.push(event))
            .unwrap();

        assert_eq!(report.status, LoadStatus::Incomplete);
        assert_eq!(report.failures.len(), 2);
        assert_eq!(
            report.repositories[0].coverage,
            RepositoryCoverage::BranchEnumerationFailed
        );
        assert_eq!(
            report.repositories[1].coverage,
            RepositoryCoverage::BranchesIncomplete { failed_branches: 1 }
        );
        assert_eq!(report.repositories[1].repository.commits[0].sha, "kept");
        assert_eq!(report.progress.repositories_processed, 2);
        assert_eq!(report.progress.branches_processed, 2);
        assert!(matches!(
            events.last(),
            Some(LoadEvent::Finished {
                status: LoadStatus::Incomplete,
                ..
            })
        ));
        assert!(events.iter().any(|event| matches!(
            event,
            LoadEvent::RepositorySnapshot {
                repository_index: 1,
                repository,
            } if repository.repository.commits.iter().any(|commit| commit.sha == "kept")
        )));
        for failure in &report.failures {
            let displayed = failure.to_string();
            assert!(!displayed.contains("secret"));
            assert!(!displayed.contains("private"));
            assert!(!displayed.contains("alpha"));
            assert!(!displayed.contains("beta"));
        }
        runner.assert_finished();
    }

    #[test]
    fn classifies_fatal_authentication_missing_gh_command_json_and_transport_failures() {
        let cases = [
            (
                failure_step("/user", vec![], 401, &[], b"credential payload"),
                FailureCategory::Authentication,
            ),
            (
                failure_step(
                    "/user",
                    vec![],
                    403,
                    &[("x-ratelimit-remaining", "0")],
                    b"rate payload",
                ),
                FailureCategory::RateLimit,
            ),
            (
                failure_step("/user", vec![], 500, &[], b"server payload"),
                FailureCategory::Api,
            ),
            (
                Step {
                    endpoint: "/user".to_owned(),
                    fields: vec![],
                    result: ScriptResult::Output(ProcessOutput {
                        success: false,
                        exit_code: Some(1),
                        stdout: BoundedBytes {
                            bytes: Vec::new(),
                            truncated: false,
                        },
                        stderr: BoundedBytes {
                            bytes: b"run gh auth login to authenticate secret-account".to_vec(),
                            truncated: false,
                        },
                    }),
                },
                FailureCategory::Authentication,
            ),
            (
                runner_error_step("/user", vec![], ProcessError::NotFound),
                FailureCategory::MissingGh,
            ),
            (
                runner_error_step("/user", vec![], ProcessError::Transport),
                FailureCategory::Transport,
            ),
            (
                Step {
                    endpoint: "/user".to_owned(),
                    fields: vec![],
                    result: ScriptResult::Output(ProcessOutput {
                        success: false,
                        exit_code: Some(2),
                        stdout: BoundedBytes {
                            bytes: Vec::new(),
                            truncated: false,
                        },
                        stderr: BoundedBytes {
                            bytes: b"opaque command failure".to_vec(),
                            truncated: false,
                        },
                    }),
                },
                FailureCategory::Command,
            ),
            (
                Step {
                    endpoint: "/user".to_owned(),
                    fields: vec![],
                    result: ScriptResult::Output(http_output(200, &[], b"not-json secret", true)),
                },
                FailureCategory::MalformedJson,
            ),
            (
                Step {
                    endpoint: "/user".to_owned(),
                    fields: vec![],
                    result: ScriptResult::Output(ProcessOutput {
                        success: true,
                        exit_code: Some(0),
                        stdout: BoundedBytes {
                            bytes: b"missing headers and secret".to_vec(),
                            truncated: false,
                        },
                        stderr: BoundedBytes {
                            bytes: Vec::new(),
                            truncated: false,
                        },
                    }),
                },
                FailureCategory::MalformedResponse,
            ),
        ];

        for (step, category) in cases {
            let runner = ScriptedRunner::new(vec![step]);
            let mut events = Vec::new();
            let failure = GitHubLoader::new(&runner)
                .load(&selection(), |event| events.push(event))
                .unwrap_err();
            assert_eq!(failure.category, category);
            assert_eq!(failure.scope, FailureScope::Authentication);
            assert!(!failure.to_string().contains("secret"));
            assert!(matches!(
                events.last(),
                Some(LoadEvent::Finished {
                    status: LoadStatus::Fatal,
                    ..
                })
            ));
            runner.assert_finished();
        }
    }

    #[test]
    fn accepts_a_launcher_notice_before_the_included_http_envelope() {
        let mut output = http_output(200, &[], br#"{"login":"Octo"}"#, true);
        let mut prefixed = b"tool launcher notice\n".to_vec();
        prefixed.extend_from_slice(&output.stdout.bytes);
        output.stdout.bytes = prefixed;
        let runner = ScriptedRunner::new(vec![
            Step {
                endpoint: "/user".to_owned(),
                fields: vec![],
                result: ScriptResult::Output(output),
            },
            repos_step(json!([])),
        ]);

        let report = GitHubLoader::new(&runner)
            .load(&selection(), |_| {})
            .expect("the HTTP envelope remains parseable after launcher output");

        assert!(report.no_owned_repositories());
        runner.assert_finished();
    }

    #[test]
    fn distinguishes_permission_from_metadata_supported_rate_limits_and_other_api_errors() {
        let headers = |remaining: &'static str| {
            let mut values = HashMap::new();
            values.insert("x-ratelimit-remaining".to_owned(), remaining.to_owned());
            values
        };
        assert_eq!(
            classify_failure(Some(403), Some(&headers("9"))),
            FailureCategory::PermissionOrNotFound
        );
        assert_eq!(
            classify_failure(Some(404), None),
            FailureCategory::PermissionOrNotFound
        );
        assert_eq!(
            classify_failure(Some(403), Some(&headers("0"))),
            FailureCategory::RateLimit
        );
        assert_eq!(
            classify_failure(Some(429), None),
            FailureCategory::RateLimit
        );
        assert_eq!(classify_failure(Some(500), None), FailureCategory::Api);
    }

    #[test]
    fn only_uses_read_only_gh_api_invocations_without_token_or_shell_arguments() {
        let runner = ScriptedRunner::new(vec![user_step(), repos_step(json!([]))]);
        GitHubLoader::new(&runner)
            .load(&selection(), |_| {})
            .unwrap();

        for arguments in runner.calls() {
            let text = arguments
                .iter()
                .map(|argument| argument.to_string_lossy())
                .collect::<Vec<_>>();
            assert_eq!(&text[..4], ["api", "--method", "GET", "--include"]);
            assert!(!text.iter().any(|argument| {
                let lower = argument.to_ascii_lowercase();
                lower.contains("token")
                    || lower.contains("authorization")
                    || matches!(
                        lower.as_str(),
                        "post" | "patch" | "put" | "delete" | "graphql"
                    )
            }));
        }
        runner.assert_finished();
    }

    fn detail_identity() -> RepositoryIdentity {
        RepositoryIdentity {
            id: 7_654_321,
            owner: "Ow ner".to_owned(),
            name: "repo#one".to_owned(),
        }
    }

    fn detail_sha() -> &'static str {
        "abcdef0123456789abcdef0123456789abcdef01"
    }

    fn detail_endpoint() -> String {
        format!("/repos/Ow%20ner/repo%23one/commits/{}", detail_sha())
    }

    fn api_file(filename: &str, changes: u64, patch: Option<&str>) -> Value {
        json!({
            "filename": filename,
            "previous_filename": null,
            "status": "modified",
            "additions": changes,
            "deletions": 0,
            "changes": changes,
            "patch": patch,
        })
    }

    #[test]
    fn commit_detail_uses_encoded_get_and_preserves_order_metadata_and_numbered_lines() {
        let patch = "@@ -10,2 +20,3 @@ heading\n-old\n+new\tvalue\u{1b}\n context\n\\ No newline at end of file";
        let runner = ScriptedRunner::new(vec![json_step(
            detail_endpoint(),
            vec![],
            json!({
                "files": [
                    {
                        "filename": "src/\u{1b}first\t.rs\r",
                        "previous_filename": "src/old\t.rs\r",
                        "status": "renamed",
                        "additions": 2,
                        "deletions": 1,
                        "changes": 3,
                        "patch": patch
                    },
                    {
                        "filename": "second.bin",
                        "previous_filename": null,
                        "status": "modified",
                        "additions": 0,
                        "deletions": 0,
                        "changes": 0
                    },
                    {
                        "filename": "third.bin",
                        "previous_filename": null,
                        "status": "removed",
                        "additions": 0,
                        "deletions": 4,
                        "changes": 4
                    }
                ]
            }),
        )]);

        let detail = GitHubLoader::new(&runner)
            .load_commit_detail(
                &detail_identity(),
                detail_sha(),
                &CancellationToken::default(),
            )
            .unwrap();

        assert_eq!(detail.files.len(), 3);
        assert_eq!(detail.files[0].path, "src/�first    .rs");
        assert_eq!(
            detail.files[0].previous_path.as_deref(),
            Some("src/old    .rs")
        );
        assert_eq!(detail.files[0].status, FileStatus::Renamed);
        assert_eq!(
            (detail.files[0].additions, detail.files[0].deletions),
            (2, 1)
        );
        let lines = detail.files[0].patch.lines();
        assert_eq!(lines.len(), 5);
        assert_eq!(lines[0].kind, DiffLineKind::Hunk);
        assert_eq!((lines[1].old_line, lines[1].new_line), (Some(10), None));
        assert_eq!((lines[2].old_line, lines[2].new_line), (None, Some(20)));
        assert_eq!(lines[2].text, "+new    value�");
        assert_eq!((lines[3].old_line, lines[3].new_line), (Some(11), Some(21)));
        assert_eq!(lines[4].kind, DiffLineKind::NoNewline);
        assert_eq!(detail.files[1].patch, PatchContent::Empty);
        assert_eq!(detail.files[2].patch, PatchContent::Unavailable);
        assert!(!detail.more_files_available);
        assert_eq!(detail.omitted_files, 0);
        assert_eq!(runner.calls().len(), 1);
        runner.assert_finished();
    }

    #[test]
    fn malformed_hunks_and_unknown_statuses_are_explicit_non_panicking_data() {
        let runner = ScriptedRunner::new(vec![json_step(
            detail_endpoint(),
            vec![],
            json!({"files": [{
                "filename": "odd.txt",
                "previous_filename": null,
                "status": "future-status",
                "additions": 1,
                "deletions": 0,
                "changes": 1,
                "patch": "@@ malformed\n+still readable"
            }]}),
        )]);

        let detail = GitHubLoader::new(&runner)
            .load_commit_detail(
                &detail_identity(),
                detail_sha(),
                &CancellationToken::default(),
            )
            .unwrap();
        assert_eq!(detail.files[0].status, FileStatus::Unknown);
        assert_eq!(detail.files[0].patch.lines()[0].kind, DiffLineKind::Other);
        assert_eq!(
            detail.files[0].patch.lines()[1].kind,
            DiffLineKind::Addition
        );
        assert_eq!(detail.files[0].patch.lines()[1].new_line, None);
        runner.assert_finished();
    }

    #[test]
    fn file_limits_bound_line_count_bytes_and_individual_line_length() {
        let oversized_line = format!("+{}", "x".repeat(MAX_DIFF_LINE_CHARS + 50));
        let many_lines = std::iter::repeat_n("+x", MAX_FILE_PATCH_LINES + 3)
            .collect::<Vec<_>>()
            .join("\n");
        let byte_heavy = std::iter::repeat_n(format!("+{}", "y".repeat(1_998)), 140)
            .collect::<Vec<_>>()
            .join("\n");
        let runner = ScriptedRunner::new(vec![json_step(
            detail_endpoint(),
            vec![],
            json!({"files": [
                api_file("long.txt", 1, Some(&oversized_line)),
                api_file("many.txt", 1, Some(&many_lines)),
                api_file("bytes.txt", 1, Some(&byte_heavy))
            ]}),
        )]);

        let detail = GitHubLoader::new(&runner)
            .load_commit_detail(
                &detail_identity(),
                detail_sha(),
                &CancellationToken::default(),
            )
            .unwrap();

        for file in &detail.files {
            let PatchContent::Capped {
                lines,
                omitted_bytes,
                reason,
                ..
            } = &file.patch
            else {
                panic!("expected a locally capped patch");
            };
            assert_eq!(*reason, PatchCapReason::FileLimit);
            assert!(*omitted_bytes > 0);
            assert!(lines.len() <= MAX_FILE_PATCH_LINES);
            assert!(
                lines
                    .iter()
                    .all(|line| line.text.chars().count() <= MAX_DIFF_LINE_CHARS)
            );
            assert!(
                lines.iter().map(|line| line.text.len()).sum::<usize>() <= MAX_FILE_PATCH_BYTES
            );
        }
        assert!(detail.files[0].patch.lines()[0].text.ends_with('…'));
        let PatchContent::Capped { omitted_lines, .. } = detail.files[1].patch else {
            unreachable!();
        };
        assert_eq!(omitted_lines, 3);
        runner.assert_finished();
    }

    #[test]
    fn commit_budget_stops_retaining_later_patch_text() {
        let patch = std::iter::repeat_n(format!("+{}", "z".repeat(1_998)), 130)
            .collect::<Vec<_>>()
            .join("\n");
        let files = (0..9)
            .map(|index| api_file(&format!("file-{index}.txt"), 1, Some(&patch)))
            .collect::<Vec<_>>();
        let runner = ScriptedRunner::new(vec![json_step(
            detail_endpoint(),
            vec![],
            json!({"files": files}),
        )]);

        let detail = GitHubLoader::new(&runner)
            .load_commit_detail(
                &detail_identity(),
                detail_sha(),
                &CancellationToken::default(),
            )
            .unwrap();
        let retained = detail
            .files
            .iter()
            .flat_map(|file| file.patch.lines())
            .map(|line| line.text.len())
            .sum::<usize>();
        assert!(retained <= MAX_COMMIT_PATCH_BYTES);
        assert!(matches!(
            detail.files[8].patch,
            PatchContent::Capped {
                ref lines,
                reason: PatchCapReason::CommitBudget,
                ..
            } if lines.is_empty()
        ));
        runner.assert_finished();
    }

    #[test]
    fn file_cap_and_link_header_disclose_incomplete_file_lists() {
        let files = (0..=MAX_DETAIL_FILES)
            .map(|index| api_file(&format!("file-{index}.txt"), 0, None))
            .collect::<Vec<_>>();
        let runner = ScriptedRunner::new(vec![Step {
            endpoint: detail_endpoint(),
            fields: vec![],
            result: ScriptResult::Output(http_output(
                200,
                &[("Link", "<https://api.invalid/next>; rel=\"next\"")],
                json!({"files": files}).to_string().as_bytes(),
                true,
            )),
        }]);

        let detail = GitHubLoader::new(&runner)
            .load_commit_detail(
                &detail_identity(),
                detail_sha(),
                &CancellationToken::default(),
            )
            .unwrap();
        assert_eq!(detail.files.len(), MAX_DETAIL_FILES);
        assert_eq!(detail.omitted_files, 1);
        assert!(detail.more_files_available);
        assert_eq!(detail.files[0].path, "file-0.txt");
        assert_eq!(detail.files[MAX_DETAIL_FILES - 1].path, "file-299.txt");
        runner.assert_finished();

        let runner = ScriptedRunner::new(vec![Step {
            endpoint: detail_endpoint(),
            fields: vec![],
            result: ScriptResult::Output(http_output(
                200,
                &[("link", "<https://api.invalid/next>; rel=\"next\"")],
                json!({"files": [api_file("only.txt", 0, None)]})
                    .to_string()
                    .as_bytes(),
                true,
            )),
        }]);
        let detail = GitHubLoader::new(&runner)
            .load_commit_detail(
                &detail_identity(),
                detail_sha(),
                &CancellationToken::default(),
            )
            .unwrap();
        assert_eq!(detail.files.len(), 1);
        assert_eq!(detail.omitted_files, 0);
        assert!(detail.more_files_available);
        runner.assert_finished();
    }

    #[test]
    fn accepts_complete_lowercase_sha256_object_ids() {
        let sha = "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789";
        let runner = ScriptedRunner::new(vec![json_step(
            format!("/repos/Ow%20ner/repo%23one/commits/{sha}"),
            vec![],
            json!({"files": []}),
        )]);

        let detail = GitHubLoader::new(&runner)
            .load_commit_detail(&detail_identity(), sha, &CancellationToken::default())
            .unwrap();
        assert!(detail.files.is_empty());
        runner.assert_finished();
    }

    #[test]
    fn response_truncation_and_malformed_detail_json_are_distinct() {
        let mut truncated = http_output(200, &[], br#"{"files":[]}"#, true);
        truncated.stdout.truncated = true;
        let cases = [
            (
                Step {
                    endpoint: detail_endpoint(),
                    fields: vec![],
                    result: ScriptResult::Output(truncated),
                },
                DetailFailure::ResponseTruncated,
            ),
            (
                Step {
                    endpoint: detail_endpoint(),
                    fields: vec![],
                    result: ScriptResult::Output(http_output(200, &[], b"not json", true)),
                },
                DetailFailure::Load(LoadFailure {
                    category: FailureCategory::MalformedJson,
                    scope: FailureScope::CommitDetail,
                    http_status: Some(200),
                }),
            ),
            (
                json_step(
                    detail_endpoint(),
                    vec![],
                    json!({"files": [{"filename": 1}]}),
                ),
                DetailFailure::Load(LoadFailure {
                    category: FailureCategory::MalformedJson,
                    scope: FailureScope::CommitDetail,
                    http_status: Some(200),
                }),
            ),
        ];

        for (step, expected) in cases {
            let runner = ScriptedRunner::new(vec![step]);
            let failure = GitHubLoader::new(&runner)
                .load_commit_detail(
                    &detail_identity(),
                    detail_sha(),
                    &CancellationToken::default(),
                )
                .unwrap_err();
            assert_eq!(failure, expected);
            runner.assert_finished();
        }
    }

    #[test]
    fn detail_failures_are_categorized_and_never_disclose_request_data() {
        let private_owner = "private owner";
        let private_name = "secret#repository";
        let private_sha = "deadbeefdeadbeefdeadbeefdeadbeefdeadbeef";
        let identity = RepositoryIdentity {
            id: 991,
            owner: private_owner.to_owned(),
            name: private_name.to_owned(),
        };
        let endpoint = "/repos/private%20owner/secret%23repository/commits/deadbeefdeadbeefdeadbeefdeadbeefdeadbeef";
        let cases = [
            (
                failure_step(endpoint, vec![], 401, &[], b"secret stderr"),
                FailureCategory::Authentication,
            ),
            (
                failure_step(endpoint, vec![], 403, &[], b"secret stderr"),
                FailureCategory::PermissionOrNotFound,
            ),
            (
                failure_step(endpoint, vec![], 404, &[], b"secret stderr"),
                FailureCategory::PermissionOrNotFound,
            ),
            (
                failure_step(
                    endpoint,
                    vec![],
                    403,
                    &[("x-ratelimit-remaining", "0")],
                    b"secret stderr",
                ),
                FailureCategory::RateLimit,
            ),
            (
                Step {
                    endpoint: endpoint.to_owned(),
                    fields: vec![],
                    result: ScriptResult::Output(ProcessOutput {
                        success: false,
                        exit_code: Some(1),
                        stdout: BoundedBytes {
                            bytes: Vec::new(),
                            truncated: false,
                        },
                        stderr: BoundedBytes {
                            bytes: b"dial tcp secret.internal: no such host".to_vec(),
                            truncated: false,
                        },
                    }),
                },
                FailureCategory::Offline,
            ),
        ];

        for (step, expected_category) in cases {
            let runner = ScriptedRunner::new(vec![step]);
            let failure = GitHubLoader::new(&runner)
                .load_commit_detail(&identity, private_sha, &CancellationToken::default())
                .unwrap_err();
            let DetailFailure::Load(failure) = failure else {
                panic!("expected a categorized load failure");
            };
            assert_eq!(failure.category, expected_category);
            assert_eq!(failure.scope, FailureScope::CommitDetail);
            let displayed = failure.to_string();
            for secret in [private_owner, private_name, private_sha, "secret.internal"] {
                assert!(!displayed.contains(secret));
            }
            runner.assert_finished();
        }
    }

    #[test]
    fn invalid_and_pre_cancelled_detail_requests_never_invoke_the_runner() {
        for sha in [
            "short",
            "ABCDEF0123456789abcdef0123456789abcdef01",
            "zbcdef0123456789abcdef0123456789abcdef01",
        ] {
            let runner = ScriptedRunner::new(Vec::new());
            let failure = GitHubLoader::new(&runner)
                .load_commit_detail(&detail_identity(), sha, &CancellationToken::default())
                .unwrap_err();
            assert!(matches!(failure, DetailFailure::Load(_)));
            assert!(runner.calls().is_empty());
        }

        let runner = ScriptedRunner::new(Vec::new());
        let cancellation = CancellationToken::default();
        cancellation.cancel();
        let failure = GitHubLoader::new(&runner)
            .load_commit_detail(&detail_identity(), detail_sha(), &cancellation)
            .unwrap_err();
        assert!(matches!(
            failure,
            DetailFailure::Load(LoadFailure {
                category: FailureCategory::Cancelled,
                ..
            })
        ));
        assert!(runner.calls().is_empty());
    }

    #[test]
    fn cancellation_stops_and_reaps_an_active_process_promptly() {
        use std::time::Instant;

        let cancellation = CancellationToken::default();
        let cancelling_thread = {
            let cancellation = cancellation.clone();
            thread::spawn(move || {
                thread::sleep(Duration::from_millis(30));
                cancellation.cancel();
            })
        };
        let started = Instant::now();
        let result = CommandRunner.run_cancellable(
            OsStr::new("sleep"),
            &[OsString::from("10")],
            &cancellation,
        );
        cancelling_thread.join().unwrap();

        assert_eq!(result, Err(ProcessError::Cancelled));
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "cancelled child should be killed and reaped promptly"
        );
    }

    #[test]
    fn pre_cancelled_load_does_not_invoke_the_process_runner() {
        let runner = ScriptedRunner::new(Vec::new());
        let cancellation = CancellationToken::default();
        cancellation.cancel();
        let mut events = Vec::new();

        let failure = GitHubLoader::new(&runner)
            .load_cancellable(&selection(), &cancellation, |event| events.push(event))
            .expect_err("cancelled load stops");

        assert_eq!(failure.category, FailureCategory::Cancelled);
        assert!(runner.calls().is_empty());
        assert!(matches!(
            events.last(),
            Some(LoadEvent::Finished {
                status: LoadStatus::Fatal,
                ..
            })
        ));
    }

    impl<T: ProcessRunner + ?Sized> ProcessRunner for &T {
        fn run(
            &self,
            executable: &OsStr,
            arguments: &[OsString],
        ) -> Result<ProcessOutput, ProcessError> {
            (**self).run(executable, arguments)
        }

        fn run_cancellable(
            &self,
            executable: &OsStr,
            arguments: &[OsString],
            cancellation: &CancellationToken,
        ) -> Result<ProcessOutput, ProcessError> {
            (**self).run_cancellable(executable, arguments, cancellation)
        }
    }
}
