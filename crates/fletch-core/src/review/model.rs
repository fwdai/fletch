//! The local reviewer's findings and what becomes of them, as rows and as the
//! `PrComment` the rest of the app already renders.

use serde::{Deserialize, Serialize};

use crate::github::{PrComment, ReviewSeverity, ThreadSource};

/// One finding as the reviewer reports it (`review_submit`): where, how much
/// it matters, and what. `path` and `line` are anchors, both optional.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Finding {
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub line: Option<u32>,
    #[serde(default)]
    pub severity: Option<ReviewSeverity>,
    pub title: String,
    #[serde(default)]
    pub body: String,
}

/// Who answered a thread: the coder (`agent`) or the user (a dismissal).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReplyAuthor {
    Agent,
    User,
}

impl ReplyAuthor {
    pub fn as_str(self) -> &'static str {
        match self {
            ReplyAuthor::Agent => "agent",
            ReplyAuthor::User => "user",
        }
    }
}

/// One reply on a thread, stored as JSON in `review_threads.replies`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reply {
    pub author: ReplyAuthor,
    pub body: String,
    /// Epoch ms.
    pub at: i64,
}

/// One `review_threads` row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadRow {
    pub id: String,
    pub agent_id: String,
    pub subdir: Option<String>,
    pub round: u32,
    pub severity: Option<ReviewSeverity>,
    pub path: Option<String>,
    pub line: Option<u32>,
    pub title: String,
    pub body: String,
    pub author: String,
    pub created_at: i64,
    pub resolved_at: Option<i64>,
    pub resolved_by: Option<ReplyAuthor>,
    pub replies: Vec<Reply>,
}

impl ThreadRow {
    pub fn resolved(&self) -> bool {
        self.resolved_at.is_some()
    }

    /// The coder had the last word and the thread is still open: a push-back
    /// awaiting a person, which the ladder must not hand back to the agent.
    pub fn we_replied_last(&self) -> bool {
        !self.resolved()
            && self
                .replies
                .last()
                .is_some_and(|r| r.author == ReplyAuthor::Agent)
    }

    /// The thread as the panel, the ladder and the agent's `pr_threads` see
    /// it — the same shape a GitHub thread has.
    pub fn to_comment(&self) -> PrComment {
        let body = if self.body.trim().is_empty() {
            self.title.clone()
        } else {
            format!("{}\n\n{}", self.title, self.body)
        };
        PrComment {
            id: self.id.clone(),
            source: ThreadSource::Local,
            severity: self.severity,
            round: Some(self.round),
            author: self.author.clone(),
            is_bot: true,
            body,
            path: self.path.clone(),
            line: self.line,
            url: None,
            replies: self.replies.len() as u32,
            we_replied_last: self.we_replied_last(),
        }
    }
}
