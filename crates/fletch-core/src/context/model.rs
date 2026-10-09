//! The context layer's types: what is stored (events and their payloads), what
//! is read back (the projection as a [`Graph`]), what is compiled for an agent
//! or the UI (a [`Bundle`]), and the pipeline records around them
//! (observations, proposals).
//!
//! Everything here is plain data with `serde` derives; the SQL lives in
//! `store.rs` and the logic in `compile.rs` / `resolve.rs`.

use serde::{Deserialize, Serialize};

/// A writer-minted id: UUIDv7, so ids sort by creation time without a
/// database sequence.
pub type Id = String;

pub fn new_id() -> Id {
    uuid::Uuid::now_v7().to_string()
}

// ---------------------------------------------------------------------------
// Vocabulary

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntityKind {
    Vision,
    Goal,
    Capability,
    Feature,
    Module,
    /// Catch-all so an extractor is never stuck for a kind.
    #[default]
    Topic,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssertionKind {
    /// We chose this (or chose against it).
    Decision,
    /// A rule to follow; most often user-stated.
    Constraint,
    /// How something currently is.
    Fact,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Domain {
    Business,
    Architectural,
    Implementation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Stance {
    Adopted,
    Rejected,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Rel {
    PartOf,
    DependsOn,
    Serves,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntityStatus {
    Active,
    Archived,
    Merged,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssertionStatus {
    /// Recorded from a workspace whose branch has not merged yet.
    Provisional,
    Confirmed,
    /// The workspace that recorded it was archived without merging.
    Abandoned,
    /// It was never right (as opposed to superseded: we changed our mind).
    Retracted,
}

// ---------------------------------------------------------------------------
// The write stamp: who wrote, where it came from, which run it came out of.

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthorKind {
    User,
    Agent,
    Extractor,
    Ingester,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Author {
    pub kind: AuthorKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
}

impl Author {
    pub fn user() -> Self {
        Self {
            kind: AuthorKind::User,
            agent_id: None,
            provider: None,
        }
    }

    pub fn agent(agent_id: impl Into<String>, provider: impl Into<String>) -> Self {
        Self {
            kind: AuthorKind::Agent,
            agent_id: Some(agent_id.into()),
            provider: Some(provider.into()),
        }
    }

    pub fn ingester() -> Self {
        Self {
            kind: AuthorKind::Ingester,
            agent_id: None,
            provider: None,
        }
    }

    pub fn extractor(agent_id: impl Into<String>, provider: impl Into<String>) -> Self {
        Self {
            kind: AuthorKind::Extractor,
            agent_id: Some(agent_id.into()),
            provider: Some(provider.into()),
        }
    }
}

/// Where a record came from. Trust lives here: a rule layer decides what may
/// land without review by source kind, never by a confidence number.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceKind {
    UserTurn,
    AgentTurn,
    Pr,
    ReviewThread,
    Roadmap,
    Workflow,
    Ui,
    /// The repository itself at a commit (the bootstrap's file tree).
    Repo,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Source {
    pub kind: SourceKind,
    /// Turn id, PR number, roadmap item code, commit SHA, … — whatever names
    /// the origin.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reference: Option<String>,
}

impl Source {
    pub fn new(kind: SourceKind, reference: Option<String>) -> Self {
        Self { kind, reference }
    }

    pub fn ui() -> Self {
        Self::new(SourceKind::Ui, None)
    }
}

/// Which run of the app produced the record: enough to derive provisional /
/// abandoned later and to link a review back to the turn.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Provenance {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit_sha: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<String>,
    /// The checkout (repo subdir) the record was made in: the unit a merge
    /// or an archive settles. `None` means the workspace's primary repo.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo: Option<String>,
}

/// The caller-supplied half of every event's stamp. The store adds `id`,
/// `host_id`, `seq` and `recorded_at`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stamp {
    pub author: Author,
    pub source: Source,
    #[serde(default)]
    pub provenance: Provenance,
}

// ---------------------------------------------------------------------------
// Events

/// Current payload schema version. Bump when a payload's *shape* changes; the
/// projector must keep replaying every older version forever.
pub const PAYLOAD_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Supersede {
    pub id: Id,
    pub reasoning: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Contradict {
    pub id: Id,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EventPayload {
    /// Creates the entity, or a new revision of an existing id.
    EntityRecorded {
        id: Id,
        slug: String,
        kind: EntityKind,
        name: String,
        summary: String,
        #[serde(default)]
        aliases: Vec<String>,
        #[serde(default)]
        paths: Vec<String>,
    },
    EntityArchived {
        id: Id,
    },
    /// `id`'s edges are redirected to `into`; `id` is left in `merged` status.
    EntityMerged {
        id: Id,
        into: Id,
    },
    AssertionRecorded {
        id: Id,
        kind: AssertionKind,
        domain: Domain,
        stance: Stance,
        statement: String,
        rationale: String,
        valid_from: i64,
        #[serde(default)]
        paths: Vec<String>,
        /// At least one.
        about: Vec<Id>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        supersedes: Option<Supersede>,
        #[serde(default)]
        contradicts: Vec<Contradict>,
        status: AssertionStatus,
    },
    Linked {
        from: Id,
        to: Id,
        rel: Rel,
    },
    Unlinked {
        from: Id,
        to: Id,
        rel: Rel,
    },
    Retracted {
        assertion_id: Id,
        reason: String,
    },
    Confirmed {
        assertion_id: Id,
    },
    Abandoned {
        assertion_id: Id,
    },
    /// A ruling on a `contradicts` edge: the tension is closed with this
    /// reasoning. Neither side changes; retract or supersede do that.
    ContradictionResolved {
        a: Id,
        b: Id,
        reasoning: String,
    },
}

impl EventPayload {
    /// The `type` column: the payload's tag.
    pub fn type_name(&self) -> &'static str {
        match self {
            Self::EntityRecorded { .. } => "entity_recorded",
            Self::EntityArchived { .. } => "entity_archived",
            Self::EntityMerged { .. } => "entity_merged",
            Self::AssertionRecorded { .. } => "assertion_recorded",
            Self::Linked { .. } => "linked",
            Self::Unlinked { .. } => "unlinked",
            Self::Retracted { .. } => "retracted",
            Self::Confirmed { .. } => "confirmed",
            Self::Abandoned { .. } => "abandoned",
            Self::ContradictionResolved { .. } => "contradiction_resolved",
        }
    }
}

/// One row of `events`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Event {
    pub id: Id,
    pub project_id: String,
    pub host_id: String,
    pub seq: u64,
    pub recorded_at: i64,
    pub stamp: Stamp,
    pub v: u32,
    pub payload: EventPayload,
}

// ---------------------------------------------------------------------------
// Write inputs

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EntityInput {
    /// `None` creates; `Some` records a new revision of that entity.
    #[serde(default)]
    pub id: Option<Id>,
    pub slug: String,
    pub kind: EntityKind,
    pub name: String,
    pub summary: String,
    #[serde(default)]
    pub aliases: Vec<String>,
    #[serde(default)]
    pub paths: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssertionInput {
    pub kind: AssertionKind,
    pub domain: Domain,
    pub stance: Stance,
    pub statement: String,
    pub rationale: String,
    /// Defaults to the event's `recorded_at`.
    #[serde(default)]
    pub valid_from: Option<i64>,
    #[serde(default)]
    pub paths: Vec<String>,
    /// Entity ids; at least one. Slugs are resolved before this point.
    pub about: Vec<Id>,
    #[serde(default)]
    pub supersedes: Option<Supersede>,
    #[serde(default)]
    pub contradicts: Vec<Contradict>,
    /// The initial status: `Confirmed` when the source carries confirmation
    /// (the user said it, a PR merged, a ruling), `Provisional` otherwise.
    pub status: AssertionStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LinkChange {
    pub from: Id,
    pub to: Id,
    pub rel: Rel,
    /// `true` links, `false` unlinks.
    pub add: bool,
}

// ---------------------------------------------------------------------------
// The projection as read back

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entity {
    pub id: Id,
    pub slug: String,
    pub kind: EntityKind,
    pub name: String,
    pub summary: String,
    pub aliases: Vec<String>,
    pub paths: Vec<String>,
    pub status: EntityStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub merged_into: Option<Id>,
    pub recorded_at: i64,
    pub author: Author,
    pub source: Source,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Assertion {
    pub id: Id,
    pub kind: AssertionKind,
    pub domain: Domain,
    pub stance: Stance,
    pub statement: String,
    pub rationale: String,
    pub valid_from: i64,
    pub paths: Vec<String>,
    pub status: AssertionStatus,
    pub recorded_at: i64,
    pub author: Author,
    pub source: Source,
    pub provenance: Provenance,
    /// Entities this is about.
    pub about: Vec<Id>,
    /// The assertion this one replaced, with the reasoning given.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supersedes: Option<Supersede>,
    /// Set when a later assertion replaced this one. A head has `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub superseded_by: Option<Id>,
    /// Assertions this one is in tension with (either direction).
    pub contradicts: Vec<Id>,
}

impl Assertion {
    /// No successor at all — the structural fact. Whether an assertion
    /// *stands now* is `compile::is_current`, which also looks at status along
    /// the chain; use that for any current-view decision.
    pub fn is_head(&self) -> bool {
        self.superseded_by.is_none()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Relation {
    pub from: Id,
    pub to: Id,
    pub rel: Rel,
}

/// A `contradicts` edge as loaded: both sides, the reasoning it was recorded
/// with, and the ruling that closed it, if any.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContradictionEdge {
    pub a: Id,
    pub b: Id,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolution: Option<Resolution>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Resolution {
    pub reasoning: String,
    pub at: i64,
    pub by: Author,
}

/// One project's whole projection, loaded in one go. Small by design (a
/// project has tens to hundreds of entities), so compile and resolve are pure
/// functions over it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Graph {
    pub project_id: String,
    pub entities: Vec<Entity>,
    pub assertions: Vec<Assertion>,
    pub relations: Vec<Relation>,
    /// The ids of the assertions that stand now, per `compile::current_heads`
    /// — derived at `ContextStore::load` so the UI applies the same rule as
    /// compile without reimplementing it. Empty on a hand-built graph.
    #[serde(default)]
    pub current: Vec<Id>,
    /// Every `contradicts` edge with its reasoning and ruling. `Assertion
    /// .contradicts` keeps the ids for cheap lookups; this is the record.
    #[serde(default)]
    pub contradictions: Vec<ContradictionEdge>,
}

impl Graph {
    pub fn entity(&self, id: &str) -> Option<&Entity> {
        self.entities.iter().find(|e| e.id == id)
    }

    pub fn assertion(&self, id: &str) -> Option<&Assertion> {
        self.assertions.iter().find(|a| a.id == id)
    }

    pub fn vision(&self) -> Option<&Entity> {
        self.entities
            .iter()
            .find(|e| e.kind == EntityKind::Vision && e.status == EntityStatus::Active)
    }
}

// ---------------------------------------------------------------------------
// Compilation: what an agent or the UI gets

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompileQuery {
    /// Entry entities, as the caller named them (ids, slugs, aliases, names).
    #[serde(default)]
    pub entities: Vec<String>,
    /// Free-text query; matched against names, summaries and statements.
    #[serde(default)]
    pub query: Option<String>,
    /// Repo-relative paths the caller is working in; entities and assertions
    /// anchored under them are entry points too.
    #[serde(default)]
    pub paths: Vec<String>,
    #[serde(default)]
    pub include_history: bool,
    /// Character budget for the rendered bundle; 0 means the default.
    #[serde(default)]
    pub budget_chars: usize,
    /// Compile the spawn-time overview instead (`compile::overview`); the
    /// other fields except `budget_chars` are ignored.
    #[serde(default)]
    pub overview: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BundleEntity {
    pub entity: Entity,
    /// Why it is in the bundle.
    pub reason: EntryReason,
    pub relations: Vec<Relation>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntryReason {
    Vision,
    Named,
    Query,
    Path,
    Neighbour,
}

/// Flags compile attaches to a served assertion.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssertionFlags {
    pub provisional: bool,
    /// Ids of head assertions it is in tension with.
    pub contradicted_by: Vec<Id>,
    /// It has predecessors (history is available).
    pub has_history: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BundleAssertion {
    pub assertion: Assertion,
    pub flags: AssertionFlags,
    /// Predecessors, newest first; only with `include_history`.
    #[serde(default)]
    pub history: Vec<Assertion>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Contradiction {
    pub a: Id,
    pub b: Id,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<String>,
}

/// The compiled picture: the vision, the entities in play, the current
/// assertions about them, and what is unresolved. Serialized as-is for the
/// UI; rendered to markdown for an agent by `render`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Bundle {
    pub project_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vision: Option<Entity>,
    /// Compiled by [`crate::context::compile::overview`]: rendered as the
    /// spawn-time overview rather than a `context_get` answer.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub overview: bool,
    pub entities: Vec<BundleEntity>,
    pub assertions: Vec<BundleAssertion>,
    pub contradictions: Vec<Contradiction>,
    /// Entry references that matched nothing — the coverage signal.
    pub misses: Vec<String>,
    pub warnings: Vec<String>,
    /// Entities dropped to fit the budget, lowest-ranked first; then, in an
    /// overview that still did not fit, the constraints dropped, likewise.
    pub truncated: Vec<Id>,
}

// ---------------------------------------------------------------------------
// Pipeline: observations → proposals

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Observation {
    pub id: Id,
    pub project_id: String,
    pub source: Source,
    pub provenance: Provenance,
    pub input_hash: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan: Option<String>,
    pub created_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extracted_at: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExtractorRun {
    pub id: Id,
    pub observation_id: Id,
    pub model: String,
    pub prompt_version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tokens_in: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tokens_out: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub created_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Evidence {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<String>,
    pub quote: String,
}

/// How a proposed assertion relates to what is already recorded. Produced by
/// `resolve::classify` (or by an extractor that was shown the heads).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RelationKind {
    New,
    Confirms,
    Supersedes,
    Contradicts,
    Duplicate,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProposedRelation {
    pub kind: RelationKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<Id>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ProposalPayload {
    Entity {
        input: EntityInput,
        stamp: Stamp,
    },
    Assertion {
        input: AssertionInput,
        stamp: Stamp,
        relation: ProposedRelation,
        /// Slugs of entities this is about that were only *proposed* when
        /// the proposal was made. Resolved when it is accepted; an unresolved
        /// one means "accept that entity first".
        #[serde(default)]
        about_pending: Vec<String>,
    },
}

/// What a writer hands to `ContextService::record_decision`: the assertion,
/// how the writer says it relates to what is already recorded (if it says),
/// the evidence and pending subjects a held proposal keeps, and — when the
/// user said it — the verified quote, which *becomes* the statement: a quote
/// proves the user's words, never a writer's paraphrase of them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub input: AssertionInput,
    /// Set only by `trust::find_user_quote`. The service replaces
    /// `input.statement` with the quote and stamps the `user_turn` source.
    pub user: Option<super::trust::UserStated>,
    /// An explicit relation from the writer (an agent's `supersedes`, the
    /// extractor's model relation). `None` means "the store decides".
    pub relation: Option<ProposedRelation>,
    pub evidence: Vec<Evidence>,
    pub about_pending: Vec<String>,
    /// The observation a held proposal links back to (the extractor's run),
    /// so the trail from turn to proposal survives being held.
    pub observation_id: Option<Id>,
}

/// The outcome of `ContextStore::land`, decided by one policy for every
/// writer (the stamp's author kind):
/// - `User` writes land, confirmed;
/// - `Agent` writes land unless current heads about the same entities and
///   domain exist and the candidate names no relation — then `Related`
///   (the two-step: nothing written);
/// - `Ingester` writes (a merged PR's lines) land next to what is there;
/// - `Extractor` writes are always held (the pilot gives model output no
///   durable authority), duplicates aside.
///
/// A restatement of a current head is `Duplicate` for everyone.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum Landing {
    Recorded { id: Id, status: AssertionStatus },
    Duplicate { id: Id },
    Related { heads: Vec<Assertion> },
    Held { proposal_id: Id },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProposalStatus {
    Pending,
    /// Landed by rule without review.
    Auto,
    Accepted,
    Dismissed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DismissReason {
    Wrong,
    Trivial,
    Duplicate,
    AlreadyKnown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Proposal {
    pub id: Id,
    pub project_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observation_id: Option<Id>,
    pub payload: ProposalPayload,
    pub evidence: Vec<Evidence>,
    pub status: ProposalStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dismiss_reason: Option<DismissReason>,
    pub created_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ruled_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ruled_by: Option<Author>,
}

/// One `context_get`, for the retrieval log.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReadRecord {
    pub project_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    pub query: CompileQuery,
    pub served_entities: Vec<Id>,
    pub served_assertions: Vec<Id>,
    pub misses: Vec<String>,
    pub chars: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stats {
    pub entities: usize,
    pub assertions: usize,
    pub heads: usize,
    pub provisional: usize,
    pub contradictions: usize,
    pub pending_proposals: usize,
    pub reads: usize,
    /// Distinct miss strings over the retrieval log, most frequent first.
    pub top_misses: Vec<(String, usize)>,
}
