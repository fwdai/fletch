//! The local review loop: a hidden reviewer agent reads a coder's checkout and
//! reports findings as local review threads; the coder answers them through
//! the ordinary `resolve-comments` playbook; a re-review follows once the code
//! moves. Runs on the host, like delegations and autopilot, so it keeps going
//! with every window shut and every client sees the same state.
//!
//! The state a client renders (`ReviewState.phase`) is *derived* on every look
//! from persisted facts — a reviewer turn in flight, a queued start, the one
//! recorded [`Failure`], the code the last review read, the last [`Sighting`]
//! of the code, and the open local threads — rather than kept as a state
//! machine of its own. Two rules hold it honest:
//!
//! - **Evidence.** Whether the code is stale changes only on a fresh read of
//!   it (a [`Sighting::Read`]). A look that read nothing, or could not read
//!   the checkout, leaves that judgement where the last read put it; the
//!   facts it rests on are persisted, and a restart re-reads every checkout
//!   before concluding anything new.
//! - **One failure record.** Every failure — a scan that couldn't verify the
//!   review, a start, a round, a queued start, a restart — is one persisted
//!   [`Failure`], written by [`Tracked::record_failure`] alone and tied to
//!   the code it failed on. The automatic re-review allows one attempt per
//!   code state: the same failure on the same code never retries by itself.

use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::sync::broadcast::error::RecvError;

use super::model::{Finding, ReplyAuthor};
use super::{now_ms, persist, store, Key};
use crate::error::{Error, Result};
use crate::host::EngineCtx;
use crate::supervisor::{SpawnRequest, StatusEvent, Supervisor};
use crate::workspace::{AgentRecord, AgentStatus, AgentView, TrackedRepo, PURPOSE_REVIEWER};

/// One `ReviewState`, whenever a checkout's review changes.
pub const EVENT_CHANGED: &str = "review:changed";

/// `settings` key: review every PR as it opens. Absent reads as on.
pub const SETTING_ON_PR_OPEN: &str = "review.on_pr_open";
/// `settings` key: the custom agent that reviews. Absent or blank falls back
/// to the custom agent named [`DEFAULT_REVIEWER_NAME`].
pub const SETTING_AGENT: &str = "review.agent";
/// The starter pack's reviewer preset, matched by name (case-insensitive).
pub const DEFAULT_REVIEWER_NAME: &str = "Reviewer";

/// Automatic re-reviews one PR gets; a manual start is never budgeted.
pub const MAX_ROUNDS_PER_PR: u32 = 3;
/// A reviewer turn longer than this is abandoned, loudly.
pub const WALL_TIMEOUT_MS: i64 = 20 * 60 * 1000;
/// A PR opened longer ago than this that the watcher first sees (a host
/// restart) is not news, and not reviewed on sight.
const PR_OPENED_RECENTLY_MS: i64 = 15 * 60 * 1000;
/// The driver's look at every tracked checkout.
const TICK: Duration = Duration::from_secs(10);
/// How long after the coder's last turn a checkout with no open PR keeps
/// being scanned for staleness; past it, only a look at it (`get_review`), a
/// PR opening or a restart scans it again.
const ACTIVE_WINDOW_MS: i64 = 2 * 60 * 1000;

/// The note a review carries when the code changed while it was read.
pub const CODE_MOVED: &str = "the code changed during the review";

/// What a start refused by a round already on the checkout says. Matched by
/// a queued start's runner, which puts the start back rather than failing it.
pub const ALREADY_RUNNING: &str = "A review is already running on this checkout";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum Phase {
    #[default]
    Idle,
    /// A start waits for the coder's turn (or a delegation) to end.
    Queued,
    Reviewing,
    Findings,
    Clean,
    Stale,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Trigger {
    Manual,
    PrOpen,
    ReReview,
}

// ── evidence ─────────────────────────────────────────────────────────────────

/// What one look saw of the code.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum Sighting {
    /// The look read no git (a cheap look: a client connecting, a quiet tick).
    #[default]
    NotScanned,
    /// The checkout's git state couldn't be read at all.
    Unreadable { reason: String },
    /// The code was read: its cheap signature (HEAD, dirty paths and kinds)
    /// and what became of its content digest.
    Read {
        cheap: String,
        digest: ContentDigest,
    },
}

/// The content half of a [`Sighting::Read`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum ContentDigest {
    /// Computed (`git_state::content_digest`).
    Verified { digest: String },
    /// Needed, but couldn't be computed: the code can't be told from the
    /// reviewed code, so it reads stale.
    Unverified { reason: String },
    /// Not needed: the cheap signature already differed from the reviewed
    /// one, so the code moved whatever its content.
    Skipped,
}

/// The last [`Sighting::Read`], persisted so a restart judges on it too. A
/// *read*, not a stored code state: its digest may be [`ContentDigest::Skipped`]
/// (the cheap half alone already said the code moved). It is only ever the
/// left side of [`identity`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodeRead {
    pub cheap: String,
    pub digest: ContentDigest,
}

impl CodeRead {
    /// The complete code state this read names, or `None` for a read that
    /// skipped its digest — a skipped read is never stored as a state.
    pub fn state(&self) -> Option<CodeState> {
        let content = match &self.digest {
            ContentDigest::Verified { digest } => Content::Verified {
                digest: digest.clone(),
            },
            ContentDigest::Unverified { reason } => Content::Unverified {
                reason: reason.clone(),
            },
            ContentDigest::Skipped => return None,
        };
        Some(CodeState {
            cheap: self.cheap.clone(),
            content,
        })
    }
}

/// A code state, complete by construction: the cheap signature (HEAD, dirty
/// paths and kinds) and what its content is. The one shape every stored "this
/// code" takes — the code a review read, the code a round read, the code a
/// failure failed on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodeState {
    pub cheap: String,
    pub content: Content,
}

/// The content of a [`CodeState`]: its digest, or why it couldn't be read —
/// an unreadable state is a state of its own.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum Content {
    Verified { digest: String },
    Unverified { reason: String },
}

impl CodeState {
    /// The persisted string form, as the wire's `reviewed_signature` carries
    /// it (and as earlier builds stored it): `cheap#digest`.
    pub fn signature(&self) -> String {
        match &self.content {
            Content::Verified { digest } => format!("{}#{digest}", self.cheap),
            Content::Unverified { .. } => format!("{}#unverified", self.cheap),
        }
    }

    /// A state from its persisted string form (`cheap#digest`), for records
    /// written before states were stored whole. A string with no digest names
    /// a state whose content was never read, which no read is ever the same as.
    fn from_signature(signature: &str) -> Self {
        let (cheap, digest) = signature.rsplit_once('#').unwrap_or((signature, ""));
        let content = if digest == "unverified" {
            Content::Unverified {
                reason: "recorded by an earlier build".into(),
            }
        } else {
            Content::Verified {
                digest: digest.to_string(),
            }
        };
        CodeState {
            cheap: cheap.to_string(),
            content,
        }
    }
}

/// Whether a read is a stored code state. See [`identity`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Identity {
    Same,
    Different,
    /// The read skipped its digest and its cheap half equals the state's: only
    /// a full read can say. Never decided on.
    Unknown,
}

/// **The one answer to "is this the same code?"** — for staleness against the
/// reviewed state, the code-moved check after a report, the failure gate of
/// the automatic review, and the PR-open "already reviewed this" check.
///
/// - the cheap halves differ → `Different`;
/// - both verified → `Same` exactly when the digests are equal;
/// - both unverified, same cheap half → `Same`: the same unreadable state,
///   nothing new to read;
/// - one verified, one unverified → `Different`: readability changed, so it
///   is a new state (an unreadable file that became readable again);
/// - the read skipped its digest, same cheap half → `Unknown`.
pub fn identity(read: &CodeRead, stored: &CodeState) -> Identity {
    if read.cheap != stored.cheap {
        return Identity::Different;
    }
    match (&read.digest, &stored.content) {
        (ContentDigest::Verified { digest }, Content::Verified { digest: theirs }) => {
            if digest == theirs {
                Identity::Same
            } else {
                Identity::Different
            }
        }
        (ContentDigest::Unverified { .. }, Content::Unverified { .. }) => Identity::Same,
        (ContentDigest::Verified { .. }, Content::Unverified { .. })
        | (ContentDigest::Unverified { .. }, Content::Verified { .. }) => Identity::Different,
        (ContentDigest::Skipped, _) => Identity::Unknown,
    }
}

/// Reading records written by earlier builds: a code state stored as its
/// string form becomes the state it names (a review's), or no state at all (a
/// failure's cheap-only signature, which named no complete state and so
/// blocks nothing).
mod compat {
    use super::CodeState;
    use serde::{Deserialize, Deserializer};

    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Stored {
        State(CodeState),
        Signature(String),
    }

    pub fn reviewed<'de, D: Deserializer<'de>>(d: D) -> Result<Option<CodeState>, D::Error> {
        Ok(match Option::<Stored>::deserialize(d)? {
            None => None,
            Some(Stored::State(state)) => Some(state),
            Some(Stored::Signature(signature)) => Some(CodeState::from_signature(&signature)),
        })
    }

    pub fn failure_code<'de, D: Deserializer<'de>>(d: D) -> Result<Option<CodeState>, D::Error> {
        Ok(match Option::<Stored>::deserialize(d)? {
            Some(Stored::State(state)) => Some(state),
            Some(Stored::Signature(_)) | None => None,
        })
    }
}

/// What the gate of the automatic review answers. See
/// [`Tracked::automatic_review`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutoReview {
    Allow,
    Block,
    /// The last read can't settle it: read this checkout's content, then ask
    /// again.
    NeedsFullRead,
}

// ── failure ──────────────────────────────────────────────────────────────────

/// Where a failure happened, which decides the phase it derives.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FailureOrigin {
    /// A look couldn't verify the review is current (its content digest
    /// failed): `stale`.
    Scan,
    /// A round couldn't begin (no reviewer, unreadable change, …): `failed`.
    Start,
    /// A round began and didn't report (spawn, send, timeout, no report):
    /// `failed`.
    Round,
    /// A queued start couldn't begin once the coder settled: `failed`.
    Queued,
    /// A round was cut off by a restart: `failed`.
    Restart,
}

/// The one failure a checkout carries, tied to the code it failed on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Failure {
    pub reason: String,
    pub origin: FailureOrigin,
    /// The code state it failed on, when a complete one was read. The
    /// automatic review never tries that state twice; no state blocks nothing.
    #[serde(
        default,
        alias = "cheap_signature",
        deserialize_with = "compat::failure_code"
    )]
    pub code: Option<CodeState>,
    pub at_ms: i64,
}

// ── the view and the state ───────────────────────────────────────────────────

/// The reviewer a checkout has: the custom agent it was built from and the
/// hidden workspace that runs it (`agent_id`, once spawned).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewerView {
    pub custom_agent_id: String,
    pub name: String,
    pub provider: String,
    pub model: Option<String>,
    pub agent_id: Option<String>,
}

/// What a client renders for one checkout; the `review:changed` payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReviewState {
    pub agent_id: String,
    pub subdir: Option<String>,
    pub phase: Phase,
    pub trigger: Option<Trigger>,
    pub reviewer: Option<ReviewerView>,
    pub round: u32,
    pub open: u32,
    pub resolved: u32,
    pub since_ms: i64,
    pub reviewed_signature: Option<String>,
    /// The failure's reason, else the note.
    pub error: Option<String>,
}

/// A reviewer turn in progress. In memory only: a turn does not survive a
/// restart, and a restart records the persisted `reviewing` as a failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Flight {
    pub reviewer_agent_id: String,
    pub started_ms: i64,
    pub submitted: bool,
    pub saw_running: bool,
}

/// A queued start: why, and with which custom agent (`None` = the configured
/// reviewer).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pending {
    pub trigger: Trigger,
    pub custom_agent_id: Option<String>,
}

/// One checkout's durable review memory (`review_state`): the conclusions and
/// the facts they rest on. `#[serde(skip)]` only for what dies with the
/// process (a turn, a claim, the active window, the restart re-scan flag).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Tracked {
    pub reviewer: Option<ReviewerView>,
    /// Reviews run on this checkout, ever.
    pub round: u32,
    /// The PR the automatic rounds are counted against, and that count.
    pub pr_number: Option<u32>,
    pub pr_rounds: u32,
    /// The code the last review read.
    #[serde(alias = "reviewed_signature", deserialize_with = "compat::reviewed")]
    pub reviewed: Option<CodeState>,
    /// The code the latest round read when it began — what it reviews, and
    /// what its failure (a restart's included) failed on.
    pub round_code: Option<CodeState>,
    pub trigger: Option<Trigger>,
    /// The one failure this checkout carries ([`Self::record_failure`]).
    pub failure: Option<Failure>,
    /// A note that is not a failure: the code moved during the review, the
    /// review was stopped, an automatic round was refused by its budget.
    pub note: Option<String>,
    /// The last time the code was read, whatever has looked since.
    pub last_read: Option<CodeRead>,
    pub since_ms: i64,
    /// The phase last derived: what a restart reads a cut-off round from,
    /// and the fallback for a checkout never read since it was persisted.
    pub phase: Phase,
    /// A start asked for while the coder was mid-turn, run once it settles.
    pub pending: Option<Pending>,
    #[serde(skip)]
    pub flight: Option<Flight>,
    /// A start has claimed this checkout and is spawning its reviewer: as
    /// exclusive as a flight, before the flight exists.
    #[serde(skip)]
    pub starting: bool,
    /// Until when the checkout counts as active (the coder's last turn) and
    /// is scanned every tick even without an open PR.
    #[serde(skip)]
    pub active_until_ms: i64,
    /// Loaded from disk and not read since: the next tick reads it, so a
    /// restart re-verifies instead of trusting old facts forever.
    #[serde(skip)]
    pub needs_rescan: bool,
}

impl Tracked {
    /// The phase the facts add up to.
    ///
    /// **Invariant: whether the code is stale changes only on a
    /// [`Sighting::Read`].** It is judged on [`Self::last_read`] — the last
    /// read's facts, persisted — and nothing else; a look that read nothing
    /// ([`Sighting::NotScanned`]) or couldn't read the checkout
    /// ([`Sighting::Unreadable`]) never writes it, so it leaves the judgement
    /// where the last read put it. A checkout never read since it was loaded
    /// keeps the stale its persisted phase concluded. Findings and clean are
    /// told apart by the open thread count, read fresh on every look.
    ///
    /// Ahead of the code: a turn in flight (`reviewing`), a queued start
    /// (`queued`), then the recorded failure — a scan that couldn't verify
    /// the review reads `stale`, every other origin `failed`.
    pub fn derive(&self, open: u32) -> Phase {
        if self.flight.is_some() {
            return Phase::Reviewing;
        }
        if self.pending.is_some() {
            return Phase::Queued;
        }
        match self.failure.as_ref().map(|f| f.origin) {
            Some(FailureOrigin::Scan) => return Phase::Stale,
            Some(_) => return Phase::Failed,
            None => {}
        }
        let Some(reviewed) = &self.reviewed else {
            return Phase::Idle;
        };
        // A read skips its digest only when its cheap half differs from the
        // reviewed one, so against the reviewed state `Unknown` cannot arise
        // (pinned by a test); were it to, "not shown the same" reads stale.
        let stale = match &self.last_read {
            Some(read) => identity(read, reviewed) != Identity::Same,
            None => self.phase == Phase::Stale,
        };
        if stale {
            Phase::Stale
        } else if open > 0 {
            Phase::Findings
        } else {
            Phase::Clean
        }
    }

    /// Take in what a look saw. Only a [`Sighting::Read`] changes the facts:
    /// it becomes [`Self::last_read`], an unverified digest records a scan
    /// failure on that code, and a digest that did its job clears one.
    /// Every scan (whatever it saw) settles the restart re-scan.
    pub fn record_sighting(&mut self, sighting: &Sighting) {
        match sighting {
            Sighting::NotScanned => {}
            Sighting::Unreadable { .. } => self.needs_rescan = false,
            Sighting::Read { cheap, digest } => {
                self.needs_rescan = false;
                let read = CodeRead {
                    cheap: cheap.clone(),
                    digest: digest.clone(),
                };
                match digest {
                    ContentDigest::Unverified { reason } => self.record_failure(
                        FailureOrigin::Scan,
                        format!("couldn't verify the review is current: {reason}"),
                        read.state(),
                    ),
                    // A scan failure lasts until the code is shown to be
                    // another state than the one that couldn't be read.
                    ContentDigest::Verified { .. } | ContentDigest::Skipped => {
                        let cleared = self.failure.as_ref().is_some_and(|f| {
                            f.origin == FailureOrigin::Scan
                                && f.code.as_ref().map_or(true, |code| {
                                    identity(&read, code) == Identity::Different
                                })
                        });
                        if cleared {
                            self.failure = None;
                        }
                    }
                }
                self.last_read = Some(read);
            }
        }
    }

    /// Record a failure: the only writer of [`Self::failure`]. `code` is the
    /// complete state it failed on — a round's own read for a start, a round,
    /// a queued start or a restart, the unreadable read for a scan — or `None`
    /// when no complete state was read, which blocks nothing. Every origin but
    /// a scan also ends the round in flight and the claim, if any.
    pub fn record_failure(
        &mut self,
        origin: FailureOrigin,
        reason: impl Into<String>,
        code: Option<CodeState>,
    ) {
        if origin != FailureOrigin::Scan {
            self.flight = None;
            self.starting = false;
        }
        self.failure = Some(Failure {
            reason: reason.into(),
            origin,
            code,
            at_ms: now_ms(),
        });
    }

    /// A failure of the round that began, on the code it read.
    pub fn round_failed(&mut self, reason: impl Into<String>) {
        let code = self.round_code.clone();
        self.record_failure(FailureOrigin::Round, reason, code);
    }

    /// Whether the code, as last read, may be reviewed again on its own.
    /// **One automatic attempt per code state**: the code must differ from
    /// what was reviewed (or a review was attempted and failed before any
    /// landed), and must not be the state a recorded failure failed on, of
    /// whatever origin — a change of state lifts the block by itself.
    /// Identity is never decided on the cheap half alone: when the read
    /// skipped its digest and its cheap half equals the failure's, the answer
    /// is [`AutoReview::NeedsFullRead`]. The budget per PR is checked
    /// separately.
    pub fn automatic_review(&self) -> AutoReview {
        let Some(read) = &self.last_read else {
            return AutoReview::Block;
        };
        let differs = match &self.reviewed {
            Some(reviewed) => identity(read, reviewed) != Identity::Same,
            None => self.failure.is_some(),
        };
        if !differs {
            return AutoReview::Block;
        }
        match self.failure.as_ref().and_then(|f| f.code.as_ref()) {
            None => AutoReview::Allow,
            Some(code) => match identity(read, code) {
                Identity::Same => AutoReview::Block,
                Identity::Different => AutoReview::Allow,
                Identity::Unknown => AutoReview::NeedsFullRead,
            },
        }
    }

    /// Whether a tick should read this checkout's code: never under a turn or
    /// a claim; always right after a restart, while a PR is open, or while
    /// the coder was active lately.
    pub fn wants_scan(&self, pr_open: bool, now: i64) -> bool {
        self.flight.is_none()
            && !self.starting
            && (self.needs_rescan || pr_open || now < self.active_until_ms)
    }

    /// The persisted memory as a restart finds it: no turn survived, so a
    /// round that was in flight is a failure, and every checkout is read
    /// again before anything new is concluded.
    pub fn loaded(mut self) -> Self {
        self.flight = None;
        self.starting = false;
        self.active_until_ms = 0;
        self.needs_rescan = true;
        if self.phase == Phase::Reviewing {
            let code = self.round_code.clone();
            self.record_failure(FailureOrigin::Restart, "interrupted by restart", code);
        }
        self
    }

    /// Claim the checkout for a start about to run. Refused while a round is
    /// in flight or another start holds the claim; [`Self::begin`],
    /// [`Self::release`] or a failure ends it.
    pub fn claim(&mut self) -> std::result::Result<(), &'static str> {
        if self.flight.is_some() || self.starting {
            return Err("a review is already running on this checkout");
        }
        self.starting = true;
        Ok(())
    }

    /// A claimed start that never began and is no failure (a race lost).
    pub fn release(&mut self) {
        self.starting = false;
    }

    /// The queued start, taken for running — only when the coder is `free`
    /// (idle, no delegation live) and nothing else holds the checkout.
    pub fn take_pending(&mut self, free: bool) -> Option<Pending> {
        if !free || self.flight.is_some() || self.starting {
            return None;
        }
        self.pending.take()
    }

    /// A queued start beaten by a round already on the checkout waits its
    /// turn again (one per checkout: a newer one queued meanwhile wins).
    pub fn requeue(&mut self, pending: Pending) {
        if self.pending.is_none() {
            self.pending = Some(pending);
        }
    }

    /// The report landed and the code was read again: if it is not the code
    /// the round read, the findings stand but the note says so — the phase
    /// reads stale, and an open PR's re-review picks it up. A read that
    /// couldn't verify is not "moved": its failure says what it is.
    pub fn check_code_after_report(&mut self) {
        let (Some(read), Some(reviewed)) = (&self.last_read, &self.reviewed) else {
            return;
        };
        if matches!(read.digest, ContentDigest::Unverified { .. }) {
            return;
        }
        if identity(read, reviewed) == Identity::Different {
            self.note = Some(CODE_MOVED.into());
        }
    }

    /// Queue a start behind the coder's turn. One per checkout: a second
    /// request while one waits keeps the first.
    pub fn queue(&mut self, trigger: Trigger, custom_agent_id: Option<String>) {
        if self.pending.is_none() {
            self.pending = Some(Pending {
                trigger,
                custom_agent_id,
            });
            self.note = None;
        }
    }

    /// Open a round on `code`, the state just read. Refused while one is in
    /// flight. A round that begins clears the failure: whatever failed
    /// before, this one is the checkout's news now.
    pub fn begin(
        &mut self,
        reviewer: ReviewerView,
        trigger: Trigger,
        code: CodeState,
        pr_number: Option<u32>,
        now: i64,
    ) -> std::result::Result<(), &'static str> {
        if self.flight.is_some() {
            return Err("a review is already running on this checkout");
        }
        self.starting = false;
        if self.pr_number != pr_number {
            self.pr_number = pr_number;
            self.pr_rounds = 0;
        }
        if trigger == Trigger::ReReview {
            self.pr_rounds += 1;
        }
        self.round += 1;
        self.failure = None;
        self.note = None;
        self.trigger = Some(trigger);
        self.round_code = Some(code);
        self.flight = Some(Flight {
            reviewer_agent_id: reviewer.agent_id.clone().unwrap_or_default(),
            started_ms: now,
            submitted: false,
            saw_running: false,
        });
        self.reviewer = Some(reviewer);
        Ok(())
    }

    /// The reviewer reported. Once per turn: the second report is refused.
    pub fn submitted(&mut self) -> std::result::Result<(), &'static str> {
        let Some(flight) = self.flight.as_mut() else {
            return Err("no review is in flight for this reviewer");
        };
        if flight.submitted {
            return Err("already submitted this turn");
        }
        flight.submitted = true;
        self.reviewed = self.round_code.clone();
        Ok(())
    }

    /// The reviewer's turn ended. Without a report that is a round failure.
    pub fn turn_ended(&mut self) {
        let Some(flight) = self.flight.take() else {
            return;
        };
        if !flight.submitted {
            self.round_failed("the reviewer ended its turn without submitting a review");
        }
    }

    /// Whether `pr_number` has had a round already — a PR seen open again
    /// (a watcher re-seed, a restart) is not news.
    pub fn reviewed_pr(&self, pr_number: u32) -> bool {
        self.round > 0 && self.pr_number == Some(pr_number)
    }

    /// The user stopped it: not a failure, just no review from this round —
    /// or none at all, for a start still queued.
    pub fn stop(&mut self) {
        self.flight = None;
        self.pending = None;
        self.note = Some("review stopped".into());
    }

    /// Whether an automatic round is still within this PR's budget.
    pub fn auto_budget_left(&self, pr_number: u32) -> bool {
        self.pr_number != Some(pr_number) || self.pr_rounds < MAX_ROUNDS_PER_PR
    }

    /// What the wire's `error` shows: the failure's reason, else the note.
    fn error(&self) -> Option<String> {
        self.failure
            .as_ref()
            .map(|f| f.reason.clone())
            .or_else(|| self.note.clone())
    }
}

/// The host's tracked checkouts, what each was last announced as, and what
/// each was last persisted as.
#[derive(Default)]
pub struct Table {
    inner: Mutex<BTreeMap<Key, Tracked>>,
    emitted: Mutex<BTreeMap<Key, ReviewState>>,
    saved: Mutex<BTreeMap<Key, String>>,
}

impl Table {
    pub fn keys(&self) -> Vec<Key> {
        self.inner.lock().keys().cloned().collect()
    }

    pub fn get(&self, key: &Key) -> Option<Tracked> {
        self.inner.lock().get(key).cloned()
    }

    /// Apply `f` to a checkout's memory, creating it on first touch.
    pub fn update<T>(&self, key: &Key, f: impl FnOnce(&mut Tracked) -> T) -> T {
        f(self.inner.lock().entry(key.clone()).or_default())
    }

    /// Apply `f` only to a tracked checkout.
    pub fn update_existing<T>(&self, key: &Key, f: impl FnOnce(&mut Tracked) -> T) -> Option<T> {
        self.inner.lock().get_mut(key).map(f)
    }

    pub fn remove(&self, key: &Key) -> Option<Tracked> {
        self.emitted.lock().remove(key);
        self.saved.lock().remove(key);
        self.inner.lock().remove(key)
    }

    /// The checkout whose reviewer turn `reviewer_agent_id` is running.
    pub fn key_of_reviewer(&self, reviewer_agent_id: &str) -> Option<Key> {
        self.inner
            .lock()
            .iter()
            .find(|(_, t)| {
                t.flight
                    .as_ref()
                    .is_some_and(|f| f.reviewer_agent_id == reviewer_agent_id)
            })
            .map(|(k, _)| k.clone())
    }

    /// Every checkout of `agent_id` that has a reviewer workspace.
    pub fn reviewers_of(&self, agent_id: &str) -> Vec<(Key, String)> {
        self.inner
            .lock()
            .iter()
            .filter(|(k, _)| k.0 == agent_id)
            .filter_map(|(k, t)| {
                t.reviewer
                    .as_ref()
                    .and_then(|r| r.agent_id.clone())
                    .map(|id| (k.clone(), id))
            })
            .collect()
    }

    fn keys_of(&self, agent_id: &str) -> Vec<Key> {
        self.inner
            .lock()
            .keys()
            .filter(|k| k.0 == agent_id)
            .cloned()
            .collect()
    }
}

/// The host's one table. Global like the delegation table: the reviewer's
/// mailbox op reaches it with no supervisor in hand.
pub fn global() -> &'static Table {
    static TABLE: OnceLock<Table> = OnceLock::new();
    TABLE.get_or_init(Table::default)
}

/// The cheap half of the code's signature, from the status already read:
/// HEAD plus every dirty path and its kind. Different from the reviewed one
/// means the code moved; equal means only the content can say. Not
/// autopilot's `state_signature`, which folds the threads in — a finding
/// resolved must not read as the code having moved.
pub fn cheap_signature(git: &crate::git_state::GitState) -> String {
    let mut files: Vec<String> = git
        .files
        .iter()
        .map(|f| format!("{}:{:?}", f.path, f.kind))
        .collect();
    files.sort_unstable();
    format!(
        "{}|{}",
        git.head_sha.as_deref().unwrap_or(""),
        files.join(",")
    )
}

// ── reviewer resolution ──────────────────────────────────────────────────────

/// The custom agent that reviews, as its row has it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewerSpec {
    pub custom_agent_id: String,
    pub name: String,
    pub base: String,
    pub model: Option<String>,
    pub effort: Option<String>,
}

fn spec_of(row: &serde_json::Map<String, serde_json::Value>) -> Option<ReviewerSpec> {
    let text = |k: &str| {
        row.get(k)
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    };
    Some(ReviewerSpec {
        custom_agent_id: text("id")?,
        name: text("name").unwrap_or_else(|| DEFAULT_REVIEWER_NAME.to_string()),
        base: text("base")?,
        model: text("model"),
        effort: text("effort"),
    })
}

fn custom_agent_by_id(conn: &rusqlite::Connection, id: &str) -> Option<ReviewerSpec> {
    crate::database::db_select(conn, "custom_agents", json!({ "where": { "id": id } }))
        .ok()?
        .first()
        .and_then(spec_of)
}

/// The reviewer to use: `requested`, else the `review.agent` setting, else
/// the newest custom agent named "Reviewer" — the first that resolves.
pub fn resolve_reviewer(
    conn: &rusqlite::Connection,
    requested: Option<&str>,
) -> Option<ReviewerSpec> {
    fn clean(s: Option<&str>) -> Option<&str> {
        s.map(str::trim).filter(|s| !s.is_empty())
    }
    if let Some(spec) = clean(requested).and_then(|id| custom_agent_by_id(conn, id)) {
        return Some(spec);
    }
    let setting = crate::database::get_setting(conn, SETTING_AGENT);
    if let Some(spec) = clean(setting.as_deref()).and_then(|id| custom_agent_by_id(conn, id)) {
        return Some(spec);
    }
    let rows = crate::database::db_select(
        conn,
        "custom_agents",
        json!({ "orderBy": "updated_at", "orderDirection": "desc" }),
    )
    .ok()?;
    rows.iter()
        .filter(|r| {
            r.get("name")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|n| n.trim().eq_ignore_ascii_case(DEFAULT_REVIEWER_NAME))
        })
        .find_map(spec_of)
}

/// Whether PRs are reviewed as they open. On unless switched off.
pub fn on_pr_open(conn: &rusqlite::Connection) -> bool {
    !matches!(
        crate::database::get_setting(conn, SETTING_ON_PR_OPEN).as_deref(),
        Some("false") | Some("0")
    )
}

// ── observation ──────────────────────────────────────────────────────────────

/// How much of the code a look reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Scan {
    /// No git at all: a [`Sighting::NotScanned`].
    Cached,
    /// The status, and the content digest only when the status alone can't
    /// tell the code from the reviewed code.
    Git,
    /// The status and the content digest: a signature to record.
    Full,
}

/// The facts a look read beside the code: thread counts and the PR, from the
/// database alone.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Counts {
    open: u32,
    resolved: u32,
    pr_number: Option<u32>,
    pr_open: bool,
}

fn repo_for<'a>(record: &'a AgentRecord, key: &Key) -> Option<&'a TrackedRepo> {
    match &key.1 {
        None => record.repos.first(),
        Some(subdir) => record.repos.iter().find(|r| &r.subdir == subdir),
    }
}

fn counts(sup: &Supervisor, key: &Key) -> Counts {
    let (open, resolved) = {
        let conn = sup.workspace.db().lock();
        store::counts(&conn, key).unwrap_or((0, 0))
    };
    let (pr_number, pr_open) = sup
        .workspace
        .agent(&key.0)
        .ok()
        .and_then(|r| repo_for(&r, key).cloned())
        .map(|repo| {
            (
                repo.pr_number.map(|n| n as u32),
                repo.pr_state.as_deref() == Some("open"),
            )
        })
        .unwrap_or((None, false));
    Counts {
        open,
        resolved,
        pr_number,
        pr_open,
    }
}

/// Whether a read needs the content digest: to record a state (`full`), or
/// when the cheap half alone can't tell the code from the reviewed state.
/// Skipping it is therefore only ever done when the cheap halves differ, so a
/// skipped read is `Different` from the reviewed state, never `Unknown`.
fn digest_needed(cheap: &str, full: bool, reviewed: Option<&CodeState>) -> bool {
    full || reviewed.is_some_and(|r| r.cheap == cheap)
}

/// What reading the code shows now. The content digest is read when `full`
/// asks for a signature to record, or when the status alone matches the
/// reviewed code; a status that differs is already stale, and the digest is
/// skipped.
async fn sight(sup: &Supervisor, key: &Key, full: bool) -> Sighting {
    let git = match crate::commands::get_git_state_impl(sup, &key.0, key.1.as_deref()).await {
        Ok(Some(git)) if git.blocked_config.is_empty() => git,
        Ok(Some(git)) => {
            return Sighting::Unreadable {
                reason: format!("refused config: {}", git.blocked_config.join(", ")),
            }
        }
        Ok(None) => {
            return Sighting::Unreadable {
                reason: "the checkout is not ready".into(),
            }
        }
        Err(e) => {
            return Sighting::Unreadable {
                reason: e.to_string(),
            }
        }
    };
    let cheap = cheap_signature(&git);
    let reviewed = global().get(key).and_then(|t| t.reviewed);
    if !digest_needed(&cheap, full, reviewed.as_ref()) {
        return Sighting::Read {
            cheap,
            digest: ContentDigest::Skipped,
        };
    }
    let checkout = crate::commands::agent_repo_checkout_opt(sup, &key.0, key.1.as_deref())
        .ok()
        .flatten()
        .map(|(_, checkout)| checkout);
    let digest = match checkout {
        Some(checkout) => match crate::git_state::content_digest(&checkout).await {
            Ok(digest) => ContentDigest::Verified { digest },
            Err(e) => ContentDigest::Unverified {
                reason: e.to_string(),
            },
        },
        None => ContentDigest::Unverified {
            reason: "the checkout is gone".into(),
        },
    };
    Sighting::Read { cheap, digest }
}

fn view(key: &Key, tracked: &Tracked, phase: Phase, counts: &Counts) -> ReviewState {
    ReviewState {
        agent_id: key.0.clone(),
        subdir: key.1.clone(),
        phase,
        trigger: tracked.trigger,
        reviewer: tracked.reviewer.clone(),
        round: tracked.round,
        open: counts.open,
        resolved: counts.resolved,
        since_ms: tracked.since_ms,
        reviewed_signature: tracked.reviewed.as_ref().map(CodeState::signature),
        error: tracked.error(),
    }
}

/// The one way the engine looks at a checkout: read as much as `scan` asks,
/// record what it saw into the table, derive the phase, persist the memory
/// when it changed and announce the view when it changed. Every path that
/// reads the code goes through here, so none can return with what it saw
/// unrecorded. Answers with what it saw, for the one caller that records a
/// signature from it. `None` when the checkout is not tracked.
async fn look(ctx: &Arc<EngineCtx>, sup: &Supervisor, key: &Key, scan: Scan) -> Option<Look> {
    let table = global();
    table.get(key)?;
    let sighting = match scan {
        Scan::Cached => Sighting::NotScanned,
        Scan::Git | Scan::Full => sight(sup, key, scan == Scan::Full).await,
    };
    let counts = counts(sup, key);
    let now = now_ms();
    let (state, saved) = table.update_existing(key, |t| {
        t.record_sighting(&sighting);
        let phase = t.derive(counts.open);
        if phase != t.phase {
            t.phase = phase;
            t.since_ms = now;
        }
        (
            view(key, t, phase, &counts),
            serde_json::to_string(&*t).ok(),
        )
    })?;
    if let Some(json) = saved {
        let changed = table.saved.lock().get(key) != Some(&json);
        if changed {
            if let Some(t) = table.get(key) {
                let conn = ctx.db.lock();
                match persist::save(&conn, key, &t) {
                    Ok(()) => {
                        table.saved.lock().insert(key.clone(), json);
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, agent_id = %key.0, "review state not persisted")
                    }
                }
            }
        }
    }
    let announce = {
        let mut emitted = table.emitted.lock();
        if emitted.get(key) == Some(&state) {
            false
        } else {
            emitted.insert(key.clone(), state.clone());
            true
        }
    };
    if announce {
        crate::host::emit(ctx.sink.as_ref(), EVENT_CHANGED, &state);
    }
    Some(Look {
        state,
        counts,
        sighting,
    })
}

/// What one [`look`] settled.
struct Look {
    state: ReviewState,
    counts: Counts,
    sighting: Sighting,
}

/// Whether the coder's checkout is in a box that is not running.
fn checkout_asleep(sup: &Supervisor, key: &Key) -> bool {
    crate::commands::agent_repo_checkout_opt(sup, &key.0, key.1.as_deref())
        .ok()
        .flatten()
        .is_some_and(|(_, checkout)| crate::sandbox::boxes::asleep_for_path(&checkout))
}

/// [`look`] reading the code.
async fn scan(ctx: &Arc<EngineCtx>, sup: &Supervisor, key: &Key) -> Option<ReviewState> {
    look(ctx, sup, key, Scan::Git).await.map(|l| l.state)
}

/// [`look`] reading no git: the last read's facts, fresh counts.
async fn refresh(ctx: &Arc<EngineCtx>, sup: &Supervisor, key: &Key) -> Option<ReviewState> {
    look(ctx, sup, key, Scan::Cached).await.map(|l| l.state)
}

/// Record a failure on a checkout ([`Tracked::record_failure`], on `code`)
/// and settle what it derives.
async fn fail(
    ctx: &Arc<EngineCtx>,
    sup: &Supervisor,
    key: &Key,
    origin: FailureOrigin,
    reason: String,
    code: Option<CodeState>,
) {
    global().update_existing(key, |t| t.record_failure(origin, reason, code));
    refresh(ctx, sup, key).await;
}

// ── the reviewer workspace ───────────────────────────────────────────────────

/// The reviewer workspace for `coder`'s checkout `repo`, reused when the one
/// on record is live and built from the same custom agent, else spawned
/// adopting the coder's checkout.
async fn ensure_reviewer(
    ctx: &Arc<EngineCtx>,
    sup: &Arc<Supervisor>,
    coder: &AgentRecord,
    repo: &TrackedRepo,
    spec: &ReviewerSpec,
    existing: Option<&ReviewerView>,
) -> Result<ReviewerView> {
    let checkout = repo.checkout_path(&coder.id)?;
    // Where the reviewer must run: where the coder's checkout lives
    // (`sandbox::engine_for_adopted_tree`). A reviewer on record that was
    // stamped with another engine — one made before placement followed the
    // tree, which could leave a box-stamped reviewer facing a host tree — is
    // replaced rather than resumed into a launch that has nowhere to run.
    let wanted_engine = crate::sandbox::engine_for_adopted_tree(&checkout);
    if let Some(id) = existing.and_then(|r| r.agent_id.as_deref()) {
        match sup.workspace.agent(id) {
            Ok(record)
                if record.archive.is_none()
                    && record.custom_agent_id.as_deref() == Some(spec.custom_agent_id.as_str())
                    && record
                        .sandbox_engine
                        .as_deref()
                        .and_then(crate::sandbox::EngineKind::from_setting)
                        .unwrap_or(crate::sandbox::EngineKind::SandboxExec)
                        == wanted_engine =>
            {
                if !sup.agents.lock().contains_key(id) {
                    sup.clone().resume_agent(ctx.clone(), id).await?;
                }
                return Ok(ReviewerView {
                    custom_agent_id: spec.custom_agent_id.clone(),
                    name: spec.name.clone(),
                    provider: record.provider.clone(),
                    model: record.model.clone(),
                    agent_id: Some(id.to_string()),
                });
            }
            Ok(record) if record.archive.is_none() => {
                // Built from another reviewer, or stamped with an engine that
                // cannot reach the checkout: replaced, not reused.
                if let Err(e) = sup.clone().discard_agent(id).await {
                    tracing::warn!(error = %e, reviewer = id, "old reviewer not discarded");
                }
            }
            _ => {}
        }
    }
    let profile = {
        let conn = ctx.db.lock();
        crate::agent_profile::library::resolve_custom_agent(
            &conn,
            &spec.custom_agent_id,
            &spec.base,
        )
    };
    let coder_name = coder.title.clone().unwrap_or_else(|| coder.name.clone());
    let req = SpawnRequest {
        view: AgentView::Custom,
        repo_path: repo.repo_path.clone(),
        provider: spec.base.clone(),
        name: None,
        effort: spec.effort.clone(),
        model: spec.model.clone(),
        instructions: profile.as_ref().and_then(|p| p.instructions.clone()),
        handoff: None,
        lineage: None,
        custom_agent_id: Some(spec.custom_agent_id.clone()),
        skills: profile
            .as_ref()
            .map(|p| p.skills.clone())
            .unwrap_or_default(),
        mcp_servers: profile
            .as_ref()
            .map(|p| p.mcp_servers.clone())
            .unwrap_or_default(),
        fork_base: Some(repo.base_branch().await),
        run_repo: None,
        owner_run_id: None,
        existing_workspace: Some(checkout),
        code_from: None,
        issue_ref: None,
        purpose: Some(PURPOSE_REVIEWER.to_string()),
        task: Some(format!("Review {coder_name}'s change")),
        account: None,
        // The reviewer reads the coder's checkout, nothing else of the user's.
        folder_grants: Vec::new(),
    };
    let record = sup.clone().spawn_agent(ctx.clone(), req).await?;
    Ok(ReviewerView {
        custom_agent_id: spec.custom_agent_id.clone(),
        name: spec.name.clone(),
        provider: record.provider.clone(),
        model: record.model.clone(),
        agent_id: Some(record.id),
    })
}

/// What one round asks: the change to read and how to report, plus the
/// threads still open from earlier rounds so they are judged, not repeated.
async fn round_prompt(
    sup: &Supervisor,
    coder: &AgentRecord,
    repo: &TrackedRepo,
    key: &Key,
    round: u32,
    trigger: Trigger,
) -> String {
    let checkout = repo.checkout_path(&coder.id).ok();
    let base = match &checkout {
        Some(c) => Some(repo.resolve_base(c).await),
        None => None,
    };
    let git = crate::commands::get_git_state_impl(sup, &coder.id, key.1.as_deref())
        .await
        .ok()
        .flatten();
    let base_name = base
        .as_ref()
        .map(|b| b.name.clone())
        .unwrap_or_else(|| repo.parent_branch.clone().unwrap_or_default());
    let base_sha = base
        .as_ref()
        .and_then(|b| b.fork_point.clone().or_else(|| b.tip.clone()))
        .unwrap_or_else(|| base_name.clone());
    let head = git
        .as_ref()
        .and_then(|g| g.head_sha.clone())
        .unwrap_or_else(|| "HEAD".into());
    let dirty: Vec<String> = git
        .as_ref()
        .map(|g| g.files.iter().map(|f| f.path.clone()).collect())
        .unwrap_or_default();
    let open = {
        let conn = sup.workspace.db().lock();
        store::unresolved_rows(&conn, key).unwrap_or_default()
    };

    let mut text = String::new();
    text.push_str(&format!(
        "[review] round {round} — {}\n\n",
        match trigger {
            Trigger::Manual => "requested by the user",
            Trigger::PrOpen => "the pull request was opened",
            Trigger::ReReview => "the code changed since your last review",
        }
    ));
    text.push_str(&format!(
        "Review the change in this checkout. Base: `{base_name}` at `{base_sha}`. Head: `{head}`.\n\
         See the committed change with `git diff {base_sha}...HEAD` and `git log {base_sha}..HEAD`"
    ));
    if dirty.is_empty() {
        text.push_str("; the working tree is clean.\n");
    } else {
        text.push_str(&format!(
            "; the working tree also has uncommitted changes in {} (`git status`, `git diff`), which are part of the change.\n",
            dirty.join(", ")
        ));
    }
    if open.is_empty() {
        text.push_str("\nNo earlier findings are open.\n");
    } else {
        text.push_str(
            "\nStill open from earlier rounds (do not report these again; judge the new code):\n",
        );
        for row in &open {
            let anchor = match (&row.path, row.line) {
                (Some(p), Some(l)) => format!("{p}:{l}"),
                (Some(p), None) => p.clone(),
                _ => "unanchored".into(),
            };
            let replied = if row.we_replied_last() {
                " — the author replied without resolving"
            } else {
                ""
            };
            text.push_str(&format!(
                "- [{}] {} ({anchor}){replied}\n",
                row.id, row.title
            ));
        }
    }
    text.push_str(
        "\nReport exactly once with the `review_submit` op as your instructions describe, then reply in one line.",
    );
    text
}

// ── the ops ──────────────────────────────────────────────────────────────────

/// Whether the coder is free for a review to read its checkout: not mid-turn
/// (or coming up) and no delegation live on the checkout.
fn coder_free(sup: &Supervisor, key: &Key) -> bool {
    !matches!(
        sup.status_of(&key.0),
        Some(AgentStatus::Running | AgentStatus::Spawning)
    ) && !crate::supervisor::delegation::is_live(key)
}

/// Ask for a review of `agent_id`'s checkout `subdir`. Runs now when the
/// coder is free; while it is mid-turn (a PR usually opens inside the
/// opener's turn) or a delegation is live on the checkout, the start is
/// queued and the state answers `queued` — the driver runs it once the coder
/// settles. One queued start per checkout: asking again keeps the first.
pub async fn start(
    ctx: &Arc<EngineCtx>,
    sup: &Arc<Supervisor>,
    agent_id: &str,
    subdir: Option<&str>,
    custom_agent_id: Option<&str>,
    trigger: Trigger,
) -> Result<ReviewState> {
    let coder = sup.workspace.agent(agent_id)?;
    if coder.archive.is_some() {
        return Err(Error::Other("agent is archived".into()));
    }
    let key = super::checkout_key(&coder, subdir)?;
    let table = global();
    if let Some(t) = table.get(&key) {
        if t.pending.is_some() {
            return refresh(ctx, sup, &key)
                .await
                .ok_or_else(|| Error::Other("review state vanished".into()));
        }
        if t.flight.is_some() || t.starting {
            return Err(Error::Other(ALREADY_RUNNING.into()));
        }
    }
    let resolves = {
        let conn = ctx.db.lock();
        resolve_reviewer(&conn, custom_agent_id).is_some()
    };
    if !resolves {
        return Err(Error::Other(
            "No reviewer is set up — pick one in Settings › Review".into(),
        ));
    }
    if !coder_free(sup, &key) {
        table.update(&key, |t| {
            t.queue(trigger, custom_agent_id.map(str::to_string))
        });
        return refresh(ctx, sup, &key)
            .await
            .ok_or_else(|| Error::Other("review state vanished".into()));
    }
    run_round(
        ctx,
        sup,
        &key,
        custom_agent_id,
        trigger,
        FailureOrigin::Start,
    )
    .await
}

/// Run a queued start once the coder is free, in the background: spawning a
/// reviewer takes a while and the caller (a status event, a tick) can't wait.
/// A failure is recorded by the round as a queued start's; one beaten by a
/// round already on the checkout waits again.
fn drain(ctx: &Arc<EngineCtx>, sup: &Arc<Supervisor>, key: &Key) {
    let free = coder_free(sup, key);
    let Some(pending) = global()
        .update_existing(key, |t| t.take_pending(free))
        .flatten()
    else {
        return;
    };
    let (ctx, sup, key) = (ctx.clone(), sup.clone(), key.clone());
    crate::host::spawn(async move {
        let result = run_round(
            &ctx,
            &sup,
            &key,
            pending.custom_agent_id.as_deref(),
            pending.trigger,
            FailureOrigin::Queued,
        )
        .await;
        if matches!(&result, Err(e) if e.to_string() == ALREADY_RUNNING) {
            global().update_existing(&key, |t| t.requeue(pending));
            refresh(&ctx, &sup, &key).await;
        }
    });
}

/// Run one round now: claim the checkout, read the change, spawn or resume
/// its reviewer, and send it the round's prompt. Every failure is recorded
/// here, before returning — `before` (a start's or a queued start's origin)
/// for one that stops the round from beginning, [`FailureOrigin::Round`]
/// after it began. Only a claim lost to a round already running records
/// nothing: that round is the checkout's news.
async fn run_round(
    ctx: &Arc<EngineCtx>,
    sup: &Arc<Supervisor>,
    key: &Key,
    custom_agent_id: Option<&str>,
    trigger: Trigger,
    before: FailureOrigin,
) -> Result<ReviewState> {
    let table = global();
    // Claimed first, before anything awaits: a second start meanwhile — the
    // watcher and the agent's own op both announcing one PR, a tick and a
    // click — is refused here rather than racing to the spawn.
    table
        .update(key, Tracked::claim)
        .map_err(|_| Error::Other(ALREADY_RUNNING.into()))?;
    let prefix = if before == FailureOrigin::Queued {
        "queued review not started: "
    } else {
        ""
    };
    if let Err((e, code)) = begin_round(ctx, sup, key, custom_agent_id, trigger).await {
        fail(ctx, sup, key, before, format!("{prefix}{e}"), code).await;
        return Err(e);
    }
    if let Err(e) = deliver_round(ctx, sup, key, trigger).await {
        global().update_existing(key, |t| t.round_failed(e.to_string()));
        refresh(ctx, sup, key).await;
        return Err(e);
    }
    refresh(ctx, sup, key)
        .await
        .ok_or_else(|| Error::Other("review state vanished".into()))
}

/// The part of a round before it begins: the reviewer resolves, the change
/// reads (its state is what the round records), the flight opens. An error
/// carries the complete state the start read, when it got that far — what
/// the failure is recorded on.
async fn begin_round(
    ctx: &Arc<EngineCtx>,
    sup: &Arc<Supervisor>,
    key: &Key,
    custom_agent_id: Option<&str>,
    trigger: Trigger,
) -> std::result::Result<(), (Error, Option<CodeState>)> {
    let unread = |e: Error| (e, None);
    let coder = sup.workspace.agent(&key.0).map_err(unread)?;
    if coder.archive.is_some() {
        return Err(unread(Error::Other("agent is archived".into())));
    }
    let spec = {
        let conn = ctx.db.lock();
        resolve_reviewer(&conn, custom_agent_id)
    }
    .ok_or_else(|| {
        unread(Error::Other(
            "No reviewer is set up — pick one in Settings › Review".into(),
        ))
    })?;
    let Some(Look {
        counts, sighting, ..
    }) = look(ctx, sup, key, Scan::Full).await
    else {
        return Err(unread(Error::Other("review state vanished".into())));
    };
    // The state the round records is this look's own, never an older read's:
    // a checkout that can't be read now has no change to review, and the
    // failure is recorded on the unreadable state it read.
    let code = match sighting {
        Sighting::Read { cheap, digest } => {
            let read = CodeRead { cheap, digest };
            match (&read.digest, read.state()) {
                (ContentDigest::Verified { .. }, Some(code)) => code,
                (ContentDigest::Unverified { reason }, code) => {
                    return Err((
                        Error::Other(format!("couldn't read the change: {reason}")),
                        code,
                    ))
                }
                _ => {
                    return Err(unread(Error::Other(
                        "couldn't read the change: its content was not read".into(),
                    )))
                }
            }
        }
        Sighting::Unreadable { reason } => {
            return Err(unread(Error::Other(format!(
                "couldn't read the change: {reason}"
            ))))
        }
        Sighting::NotScanned => {
            return Err(unread(Error::Other(
                "couldn't read the change: its content was not read".into(),
            )))
        }
    };
    let existing = global().get(key).and_then(|t| t.reviewer);
    let placeholder = ReviewerView {
        custom_agent_id: spec.custom_agent_id.clone(),
        name: spec.name.clone(),
        provider: spec.base.clone(),
        model: spec.model.clone(),
        agent_id: existing.as_ref().and_then(|r| r.agent_id.clone()),
    };
    let now = now_ms();
    global()
        .update(key, |t| {
            t.active_until_ms = now + ACTIVE_WINDOW_MS;
            t.begin(placeholder, trigger, code.clone(), counts.pr_number, now)
        })
        .map_err(|e| (Error::Other(e.into()), Some(code.clone())))?;
    refresh(ctx, sup, key).await;
    Ok(())
}

/// The part of a round after it begins: the reviewer workspace, the prompt.
async fn deliver_round(
    ctx: &Arc<EngineCtx>,
    sup: &Arc<Supervisor>,
    key: &Key,
    trigger: Trigger,
) -> Result<()> {
    let coder = sup.workspace.agent(&key.0)?;
    let repo = repo_for(&coder, key)
        .cloned()
        .ok_or_else(|| Error::Other("agent has no tracked repos".into()))?;
    let table = global();
    let tracked = table
        .get(key)
        .ok_or_else(|| Error::Other("review state vanished".into()))?;
    let spec_id = tracked
        .reviewer
        .as_ref()
        .map(|r| r.custom_agent_id.clone())
        .unwrap_or_default();
    let spec = {
        let conn = ctx.db.lock();
        resolve_reviewer(&conn, Some(&spec_id))
    }
    .ok_or_else(|| Error::Other("the reviewer is no longer set up".into()))?;
    let reviewer = ensure_reviewer(ctx, sup, &coder, &repo, &spec, tracked.reviewer.as_ref())
        .await
        .map_err(|e| Error::Other(format!("the reviewer could not start: {e}")))?;
    let reviewer_id = reviewer.agent_id.clone().unwrap_or_default();
    table.update(key, |t| {
        if let Some(f) = t.flight.as_mut() {
            f.reviewer_agent_id = reviewer_id.clone();
        }
        t.reviewer = Some(reviewer.clone());
    });
    let prompt = round_prompt(sup, &coder, &repo, key, tracked.round, trigger).await;
    let turn_id = uuid::Uuid::new_v4().to_string();
    sup.clone()
        .send_user_message(ctx, &reviewer_id, &turn_id, &prompt, &[])
        .await
        .map(|_| ())
        .map_err(|e| Error::Other(format!("the review could not be sent: {e}")))
}

/// Stop the reviewer turn running on `agent_id`'s checkout, or cancel a
/// start queued for it, if any.
pub async fn stop(
    ctx: &Arc<EngineCtx>,
    sup: &Arc<Supervisor>,
    agent_id: &str,
    subdir: Option<&str>,
) -> Result<ReviewState> {
    let coder = sup.workspace.agent(agent_id)?;
    let key = super::checkout_key(&coder, subdir)?;
    let table = global();
    let tracked = table.get(&key);
    let reviewer = tracked
        .as_ref()
        .and_then(|t| t.flight.as_ref().map(|f| f.reviewer_agent_id.clone()));
    let queued = tracked.as_ref().is_some_and(|t| t.pending.is_some());
    if let Some(id) = &reviewer {
        if !id.is_empty() {
            if let Err(e) = sup.clone().stop_agent(ctx.clone(), id).await {
                tracing::warn!(error = %e, reviewer = %id, "reviewer not interrupted");
            }
        }
    }
    if reviewer.is_some() || queued {
        table.update(&key, Tracked::stop);
    }
    refresh(ctx, sup, &key)
        .await
        .ok_or_else(|| Error::Other("no review on this checkout".into()))
}

pub async fn get(
    ctx: &Arc<EngineCtx>,
    sup: &Arc<Supervisor>,
    agent_id: &str,
    subdir: Option<&str>,
) -> Result<Option<ReviewState>> {
    let coder = sup.workspace.agent(agent_id)?;
    let key = super::checkout_key(&coder, subdir)?;
    Ok(scan(ctx, sup, &key).await)
}

/// Every tracked checkout, from the facts on record: no git per checkout for
/// a client connecting; the next tick reads what needs reading.
pub async fn all(ctx: &Arc<EngineCtx>, sup: &Arc<Supervisor>) -> Vec<ReviewState> {
    let mut out = Vec::new();
    for key in global().keys() {
        if let Some(state) = refresh(ctx, sup, &key).await {
            out.push(state);
        }
    }
    out
}

/// The user dismissed a local thread: resolved as theirs, the reason kept.
pub async fn dismiss_thread(
    ctx: &Arc<EngineCtx>,
    sup: &Arc<Supervisor>,
    agent_id: &str,
    subdir: Option<&str>,
    thread_id: &str,
    reason: Option<&str>,
) -> Result<()> {
    let coder = sup.workspace.agent(agent_id)?;
    let key = super::checkout_key(&coder, subdir)?;
    {
        let conn = ctx.db.lock();
        let Some(row) = store::get(&conn, thread_id)? else {
            return Err(Error::Other(format!(
                "no local review thread {thread_id:?}"
            )));
        };
        if row.agent_id != key.0 || row.subdir != key.1 {
            return Err(Error::Other(format!(
                "thread {thread_id:?} is not on this checkout"
            )));
        }
        let note = reason
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or("dismissed");
        store::reply(&conn, thread_id, ReplyAuthor::User, note)?;
        store::resolve(&conn, thread_id, ReplyAuthor::User)?;
    }
    super::emit_threads_changed(ctx, &key.0, key.1.as_deref(), Vec::new()).await;
    refresh(ctx, sup, &key).await;
    Ok(())
}

/// The reviewer's report (`review_submit`), by the reviewer's own agent id.
pub async fn submit(
    ctx: &Arc<EngineCtx>,
    sup: &Arc<Supervisor>,
    reviewer_agent_id: &str,
    verdict: &str,
    findings: Vec<Finding>,
) -> Result<()> {
    let record = sup.workspace.agent(reviewer_agent_id)?;
    if record.purpose.as_deref() != Some(PURPOSE_REVIEWER) {
        return Err(Error::Other(
            "review_submit is for the reviewer agent only".into(),
        ));
    }
    let table = global();
    let key = table
        .key_of_reviewer(reviewer_agent_id)
        .ok_or_else(|| Error::Other("no review is in flight for this reviewer".into()))?;
    // The findings decide: an approval with findings is a revise, as a revise
    // with none is an approval — nothing reported is ever dropped.
    if !matches!(verdict, "approve" | "revise") {
        return Err(Error::Other(format!(
            "verdict must be \"approve\" or \"revise\", got {verdict:?}"
        )));
    }
    let approve = findings.is_empty();
    let (round, author) = table
        .update_existing(&key, |t| {
            t.submitted().map(|()| {
                (
                    t.round,
                    t.reviewer
                        .as_ref()
                        .map(|r| r.name.clone())
                        .unwrap_or_else(|| DEFAULT_REVIEWER_NAME.into()),
                )
            })
        })
        .ok_or_else(|| Error::Other("no review is in flight for this reviewer".into()))?
        .map_err(|e| Error::Other(e.into()))?;
    if !approve {
        let ids = {
            let conn = ctx.db.lock();
            store::insert_findings(&conn, &key, round, &author, &findings)?
        };
        super::emit_threads_changed(ctx, &key.0, key.1.as_deref(), ids).await;
    }
    // The code the reviewer read was recorded when the round began. Read it
    // again now (recorded like every look): code that moved since keeps the
    // findings and carries the note; a digest that fails records a scan
    // failure, so the checkout reads stale with why.
    look(ctx, sup, &key, Scan::Full).await;
    table.update_existing(&key, Tracked::check_code_after_report);
    refresh(ctx, sup, &key).await;
    Ok(())
}

// ── triggers ─────────────────────────────────────────────────────────────────

/// A PR of `agent_id`'s checkout opened (through the agent's `open_pr`, or
/// seen by the watcher): review it, when the setting says so, nothing is in
/// flight, this PR has not had a round, and the code is not the code last
/// reviewed. `opened_at` lets a watcher's first look at a long-open PR (a
/// restart) pass quietly.
pub async fn on_pr_opened(
    ctx: &Arc<EngineCtx>,
    sup: &Arc<Supervisor>,
    agent_id: &str,
    subdir: Option<&str>,
    number: Option<u32>,
    opened_at: Option<i64>,
) {
    if opened_at.is_some_and(|at| now_ms() - at > PR_OPENED_RECENTLY_MS) {
        return;
    }
    let on = {
        let conn = ctx.db.lock();
        on_pr_open(&conn)
    };
    if !on {
        return;
    }
    let Ok(coder) = sup.workspace.agent(agent_id) else {
        return;
    };
    if coder.purpose.is_some() || coder.owner_run_id.is_some() {
        return;
    }
    let Ok(key) = super::checkout_key(&coder, subdir) else {
        return;
    };
    let tracked = global().get(&key);
    if tracked
        .as_ref()
        .is_some_and(|t| t.flight.is_some() || t.starting)
    {
        return;
    }
    if let (Some(t), Some(n)) = (&tracked, number) {
        if t.reviewed_pr(n) {
            return;
        }
    }
    // Reviewed clean or with findings at exactly this code (a review asked
    // for before the PR): nothing new to read.
    if tracked.as_ref().is_some_and(|t| t.reviewed.is_some()) {
        look(ctx, sup, &key, Scan::Git).await;
        let current = global()
            .get(&key)
            .is_some_and(|t| match (&t.last_read, &t.reviewed) {
                (Some(read), Some(reviewed)) => identity(read, reviewed) == Identity::Same,
                _ => false,
            });
        if current {
            return;
        }
    }
    if let Err(e) = start(ctx, sup, agent_id, subdir, None, Trigger::PrOpen).await {
        tracing::info!(error = %e, agent_id, "review on PR open not started");
    }
}

/// The coder is being archived or discarded: its reviewers go with it, and
/// so does the checkout's review memory.
///
/// `ctx` is what an archive needs; a discard (which has none in hand) discards
/// the reviewers too.
pub async fn on_coder_gone(sup: &Arc<Supervisor>, ctx: Option<&Arc<EngineCtx>>, agent_id: &str) {
    let table = global();
    let reviewers = table.reviewers_of(agent_id);
    for key in table.keys_of(agent_id) {
        table.remove(&key);
    }
    {
        let conn = sup.workspace.db().lock();
        if let Err(e) = persist::delete_for_agent(&conn, agent_id) {
            tracing::warn!(error = %e, agent_id, "review state rows not deleted");
        }
    }
    for (_, reviewer) in reviewers {
        let Ok(record) = sup.workspace.agent(&reviewer) else {
            continue;
        };
        if ctx.is_some() && record.archive.is_some() {
            continue;
        }
        let result = clean_up_reviewer(sup.clone(), ctx.cloned(), reviewer.clone()).await;
        if let Err(e) = result {
            tracing::warn!(error = %e, reviewer = %reviewer, "reviewer not cleaned up with its coder");
        }
    }
}

/// Archive (with a ctx) or discard (without) one reviewer workspace. A boxed,
/// concrete future on purpose: the archive and the discard each come back
/// through [`on_coder_gone`] for the reviewer's own (empty) set of reviewers,
/// and an `async fn` recursing through an opaque type cannot be sent.
fn clean_up_reviewer(
    sup: Arc<Supervisor>,
    ctx: Option<Arc<EngineCtx>>,
    reviewer: String,
) -> Pin<Box<dyn Future<Output = Result<()>> + Send>> {
    Box::pin(async move {
        match ctx {
            None => sup.discard_agent(&reviewer).await,
            Some(ctx) => {
                sup.archive_agent_as(ctx, &reviewer, crate::supervisor::ArchiveTrigger::User)
                    .await
            }
        }
    })
}

// ── the driver ───────────────────────────────────────────────────────────────

/// Load the persisted memory as a restart finds it ([`Tracked::loaded`]): a
/// round in flight at the last shutdown is a restart failure, and every
/// checkout is read again on the first tick.
pub fn load(ctx: &EngineCtx) {
    let rows = {
        let conn = ctx.db.lock();
        persist::load_all(&conn)
    };
    let rows = match rows {
        Ok(rows) => rows,
        Err(e) => {
            tracing::warn!(error = %e, "review state not loaded");
            return;
        }
    };
    let table = global();
    for (key, tracked) in rows {
        let tracked = tracked.loaded();
        table.update(&key, |t| *t = tracked);
    }
}

async fn on_status(ctx: &Arc<EngineCtx>, sup: &Arc<Supervisor>, event: StatusEvent) {
    let table = global();
    if let Some(key) = table.key_of_reviewer(&event.agent_id) {
        let ended = table
            .update_existing(&key, |t| {
                let Some(flight) = t.flight.as_mut() else {
                    return false;
                };
                match event.status {
                    AgentStatus::Running => {
                        flight.saw_running = true;
                        false
                    }
                    AgentStatus::Spawning => false,
                    AgentStatus::Idle if !flight.saw_running => false,
                    AgentStatus::Idle => {
                        t.turn_ended();
                        true
                    }
                    AgentStatus::Stopped => {
                        t.round_failed("the reviewer was stopped before it reported");
                        true
                    }
                    AgentStatus::Error => {
                        t.round_failed("the reviewer's turn failed");
                        true
                    }
                }
            })
            .unwrap_or(false);
        if ended {
            refresh(ctx, sup, &key).await;
        }
        return;
    }
    // A coder settling: its checkouts may have moved, or resolved a thread —
    // and stay worth scanning for a while.
    if event.status == AgentStatus::Idle {
        let until = now_ms() + ACTIVE_WINDOW_MS;
        for key in table.keys_of(&event.agent_id) {
            table.update_existing(&key, |t| t.active_until_ms = until);
            scan(ctx, sup, &key).await;
            drain(ctx, sup, &key);
        }
    }
}

/// The status stream lagged: whatever the in-flight reviewers' turns did
/// meanwhile is read back from their current status.
async fn resync(ctx: &Arc<EngineCtx>, sup: &Arc<Supervisor>) {
    let table = global();
    for key in table.keys() {
        let Some(flight) = table.get(&key).and_then(|t| t.flight) else {
            continue;
        };
        if flight.reviewer_agent_id.is_empty() {
            continue;
        }
        let status = sup.status_of(&flight.reviewer_agent_id);
        let ended = table
            .update_existing(&key, |t| match status {
                // Reported and settled, or settled after it was seen running:
                // the turn is over. Idle without either is ambiguous (it may
                // not have started) and is left to the wall clock.
                Some(AgentStatus::Idle) if flight.submitted || flight.saw_running => {
                    t.turn_ended();
                    true
                }
                Some(AgentStatus::Error) => {
                    t.round_failed("the reviewer's turn failed");
                    true
                }
                Some(AgentStatus::Stopped) | None => {
                    t.round_failed("the reviewer went away before it reported");
                    true
                }
                _ => false,
            })
            .unwrap_or(false);
        if ended {
            refresh(ctx, sup, &key).await;
        }
    }
}

/// One look at every tracked checkout: timeouts, staleness, and the
/// automatic re-review an open PR gets — one attempt per code state, within
/// its budget ([`Tracked::wants_automatic_review`]). A checkout's code is
/// read only when [`Tracked::wants_scan`] says so; the rest settle from the
/// facts on record. A start runs in the background — spawning a reviewer
/// takes a while, and the other checkouts can't wait.
async fn tick(ctx: &Arc<EngineCtx>, sup: &Arc<Supervisor>) {
    let table = global();
    let now = now_ms();
    let on = {
        let conn = ctx.db.lock();
        on_pr_open(&conn)
    };
    for key in table.keys() {
        // The wall clock: a reported turn that never settled is over; one
        // that never reported has failed, and is interrupted.
        let timed_out = table
            .update_existing(&key, |t| match &t.flight {
                Some(f) if now - f.started_ms > WALL_TIMEOUT_MS => {
                    let id = f.reviewer_agent_id.clone();
                    if f.submitted {
                        t.turn_ended();
                        None
                    } else {
                        t.round_failed("the reviewer did not report within 20 minutes");
                        Some(id)
                    }
                }
                _ => None,
            })
            .flatten();
        if let Some(id) = timed_out {
            if !id.is_empty() {
                let _ = sup.clone().stop_agent(ctx.clone(), &id).await;
            }
        }
        // A queued start whose coder settled without a status event reaching
        // here (a delegation that ended after the turn did): run it now.
        drain(ctx, sup, &key);
        let Some(tracked) = table.get(&key) else {
            continue;
        };
        let pr_open = counts(sup, &key).pr_open;
        // A tick does not wake a sleeping box to scan it: the last read
        // stands, and the scan it still wants runs once the box is up.
        let scan = if tracked.wants_scan(pr_open, now) && !checkout_asleep(sup, &key) {
            Scan::Git
        } else {
            Scan::Cached
        };
        let Some(Look { counts, .. }) = look(ctx, sup, &key, scan).await else {
            continue;
        };
        if !on {
            continue;
        }
        let Some(pr_number) = counts.pr_number.filter(|_| counts.pr_open) else {
            continue;
        };
        let Some(mut tracked) = table.get(&key) else {
            continue;
        };
        if tracked.flight.is_some()
            || tracked.starting
            || tracked.pending.is_some()
            || !coder_free(sup, &key)
        {
            continue;
        }
        // Never decided on a skipped digest: read the content of this one
        // key (it otherwise wants a review, so the scan budget holds), then
        // ask again on that read.
        let mut gate = tracked.automatic_review();
        if gate == AutoReview::NeedsFullRead {
            look(ctx, sup, &key, Scan::Full).await;
            let Some(read_again) = table.get(&key) else {
                continue;
            };
            tracked = read_again;
            gate = tracked.automatic_review();
        }
        if gate != AutoReview::Allow {
            continue;
        }
        let resolves = {
            let conn = ctx.db.lock();
            resolve_reviewer(&conn, None).is_some()
        };
        if !resolves {
            table.update_existing(&key, |t| {
                t.note = Some("no reviewer is set up for the automatic re-review".into())
            });
            refresh(ctx, sup, &key).await;
            continue;
        }
        if !tracked.auto_budget_left(pr_number) {
            let why =
                format!("re-review budget spent for PR #{pr_number} ({MAX_ROUNDS_PER_PR} rounds)");
            table.update_existing(&key, |t| t.note = Some(why));
            refresh(ctx, sup, &key).await;
            continue;
        }
        let (ctx, sup, key) = (ctx.clone(), sup.clone(), key.clone());
        crate::host::spawn(async move {
            // A round records its own failures. A refusal before it — no
            // reviewer set up — is recorded here, on this code, so the same
            // state is not retried every tick.
            let result = start(
                &ctx,
                &sup,
                &key.0,
                key.1.as_deref(),
                None,
                Trigger::ReReview,
            )
            .await;
            if let Err(e) = result {
                let recorded = global()
                    .get(&key)
                    .and_then(|t| t.failure)
                    .is_some_and(|f| f.at_ms >= now);
                if !recorded && e.to_string() != ALREADY_RUNNING {
                    let code = global()
                        .get(&key)
                        .and_then(|t| t.last_read)
                        .and_then(|r| r.state());
                    fail(
                        &ctx,
                        &sup,
                        &key,
                        FailureOrigin::Start,
                        format!("re-review not started: {e}"),
                        code,
                    )
                    .await;
                }
            }
        });
    }
}

/// Start the driver: the persisted memory, then a look on every status
/// transition and every [`TICK`].
pub fn spawn(ctx: Arc<EngineCtx>, sup: Arc<Supervisor>) {
    load(&ctx);
    crate::host::spawn(async move {
        let mut statuses = sup.subscribe_status();
        // One clock outside the loop: a sleep built per iteration would be
        // dropped and restarted by every status event, and a busy host would
        // never tick.
        let mut ticker = tokio::time::interval(TICK);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                event = statuses.recv() => match event {
                    Ok(event) => on_status(&ctx, &sup, event).await,
                    Err(RecvError::Lagged(_)) => resync(&ctx, &sup).await,
                    Err(RecvError::Closed) => return,
                },
                _ = ticker.tick() => tick(&ctx, &sup).await,
            }
        }
    });
}

#[cfg(test)]
#[path = "engine_tests.rs"]
mod tests;
