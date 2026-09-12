use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::day::DaySelection;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Inbox {
    pub source: InboxSource,
    pub repositories: Vec<Repository>,
}

impl Inbox {
    pub fn demo(repositories: Vec<Repository>) -> Self {
        Self {
            source: InboxSource::Demo,
            repositories,
        }
    }

    pub fn live(selection: DaySelection) -> Self {
        Self {
            source: InboxSource::Live { selection },
            repositories: Vec::new(),
        }
    }

    pub fn child_panes_available(&self) -> bool {
        matches!(self.source, InboxSource::Demo)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum InboxSource {
    Demo,
    Live { selection: DaySelection },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Repository {
    pub name: String,
    pub commits: Vec<Commit>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Commit {
    pub sha: String,
    pub subject: String,
    pub author: GitHubAuthor,
    pub authored_at: DateTime<Utc>,
    pub files: ChildPane<FileChange>,
}

impl Commit {
    pub fn display_sha(&self) -> String {
        self.sha.chars().take(7).collect()
    }

    pub fn label(&self) -> String {
        format!("{}  {}", self.display_sha(), self.subject)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitHubAuthor {
    pub login: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileChange {
    pub path: String,
    pub diff_lines: ChildPane<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChildPane<T> {
    Available(Vec<T>),
    Unavailable,
}

impl<T> ChildPane<T> {
    pub fn as_slice(&self) -> &[T] {
        match self {
            Self::Available(values) => values,
            Self::Unavailable => &[],
        }
    }
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};

    use super::*;

    #[test]
    fn full_sha_is_retained_separately_from_its_display_abbreviation() {
        let commit = Commit {
            sha: "0123456789abcdef0123456789abcdef01234567".to_owned(),
            subject: "Keep the source identity".to_owned(),
            author: GitHubAuthor {
                login: "octo".to_owned(),
            },
            authored_at: Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap(),
            files: ChildPane::Unavailable,
        };
        assert_eq!(commit.display_sha(), "0123456");
        assert_eq!(commit.sha, "0123456789abcdef0123456789abcdef01234567");
        assert_eq!(commit.author.login, "octo");
    }
}
