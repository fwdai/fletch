// Project context DTOs — the TypeScript mirror of the Rust `context::model`
// (crates/fletch-core/src/context/model.rs). These match the serde JSON
// exactly: snake_case enums, `null`-less optionals (an absent `Option` is
// skipped, so it arrives as `undefined`).

export type EntityKind = "vision" | "goal" | "capability" | "feature" | "module" | "topic";
export type AssertionKind = "decision" | "constraint" | "fact";
export type Domain = "business" | "architectural" | "implementation";
export type Stance = "adopted" | "rejected";
export type Rel = "part_of" | "depends_on" | "serves";
export type EntityStatus = "active" | "archived" | "merged";
export type AssertionStatus = "provisional" | "confirmed" | "abandoned" | "retracted";

export type AuthorKind = "user" | "agent" | "extractor" | "ingester";

export interface Author {
  kind: AuthorKind;
  agent_id?: string;
  provider?: string;
}

export type SourceKind =
  | "user_turn"
  | "agent_turn"
  | "pr"
  | "review_thread"
  | "roadmap"
  | "workflow"
  | "ui"
  | "repo";

export interface Source {
  kind: SourceKind;
  reference?: string;
}

export interface Provenance {
  workspace_id?: string;
  branch?: string;
  commit_sha?: string;
  session_id?: string;
  turn_id?: string;
  /** The checkout (repo subdir) the record was made in; absent means the
   *  workspace's primary repo. */
  repo?: string;
}

export interface Stamp {
  author: Author;
  source: Source;
  provenance: Provenance;
}

export interface Supersede {
  id: string;
  reasoning: string;
}

export interface Contradict {
  id: string;
  reasoning?: string;
}

/** The write input for an entity; `id` set records a revision. */
export interface EntityInput {
  id?: string;
  slug: string;
  kind: EntityKind;
  name: string;
  summary: string;
  aliases: string[];
  paths: string[];
}

/** The write input for an assertion. The host forces `status: confirmed` for
 *  UI writes, so callers send it as a formality. */
export interface AssertionInput {
  kind: AssertionKind;
  domain: Domain;
  stance: Stance;
  statement: string;
  rationale: string;
  valid_from?: number;
  paths: string[];
  /** Entity ids; at least one. */
  about: string[];
  supersedes?: Supersede;
  contradicts: Contradict[];
  status: AssertionStatus;
}

export interface LinkChange {
  from: string;
  to: string;
  rel: Rel;
  /** `true` links, `false` unlinks. */
  add: boolean;
}

export interface ContextEntity {
  id: string;
  slug: string;
  kind: EntityKind;
  name: string;
  summary: string;
  aliases: string[];
  paths: string[];
  status: EntityStatus;
  merged_into?: string;
  recorded_at: number;
  author: Author;
  source: Source;
}

export interface ContextAssertion {
  id: string;
  kind: AssertionKind;
  domain: Domain;
  stance: Stance;
  statement: string;
  rationale: string;
  valid_from: number;
  paths: string[];
  status: AssertionStatus;
  recorded_at: number;
  author: Author;
  source: Source;
  provenance: Provenance;
  about: string[];
  supersedes?: Supersede;
  /** Set when a later assertion replaced this one; a head has none. */
  superseded_by?: string;
  contradicts: string[];
}

export interface ContextRelation {
  from: string;
  to: string;
  rel: Rel;
}

/** The ruling that closed a `contradicts` edge. Neither side changes. */
export interface Resolution {
  reasoning: string;
  at: number;
  by: Author;
}

/** A `contradicts` edge as loaded: both sides, the reasoning it was recorded
 *  with, and the ruling that closed it, if any. */
export interface ContradictionEdge {
  a: string;
  b: string;
  reasoning?: string;
  resolution?: Resolution;
}

export interface ContextGraph {
  project_id: string;
  entities: ContextEntity[];
  assertions: ContextAssertion[];
  relations: ContextRelation[];
  /** Ids of the assertions that stand now (`compile::current_heads`), derived by the host. */
  current: string[];
  /** Every `contradicts` edge with its reasoning and ruling — the record;
   *  `ContextAssertion.contradicts` only keeps the ids. */
  contradictions: ContradictionEdge[];
}

export interface CompileQuery {
  entities: string[];
  query?: string;
  paths: string[];
  include_history: boolean;
  budget_chars: number;
  /** Compile the spawn-time overview every agent's instructions carry
   *  instead; the other fields except `budget_chars` are ignored. */
  overview?: boolean;
}

export interface Evidence {
  session_id?: string;
  turn_id?: string;
  quote: string;
}

export type RelationKind = "new" | "confirms" | "supersedes" | "contradicts" | "duplicate";

export interface ProposedRelation {
  kind: RelationKind;
  target?: string;
  reasoning?: string;
}

export type ProposalPayload =
  | { type: "entity"; input: EntityInput; stamp: Stamp }
  | {
      type: "assertion";
      input: AssertionInput;
      stamp: Stamp;
      relation: ProposedRelation;
      /** Slugs of subjects that were only proposed when this was made; one
       *  without an accepted entity yet means "accept that entity first". */
      about_pending: string[];
    };

export type ProposalStatus = "pending" | "auto" | "accepted" | "dismissed";
export type DismissReason = "wrong" | "trivial" | "duplicate" | "already_known";

export interface ContextProposal {
  id: string;
  project_id: string;
  observation_id?: string;
  payload: ProposalPayload;
  evidence: Evidence[];
  status: ProposalStatus;
  dismiss_reason?: DismissReason;
  created_at: number;
  ruled_at?: number;
  ruled_by?: Author;
}

export interface ContextStats {
  entities: number;
  assertions: number;
  heads: number;
  provisional: number;
  contradictions: number;
  pending_proposals: number;
  reads: number;
  top_misses: [string, number][];
}

/** `context_overview`: everything the Context tab shows in one read. */
export interface ContextOverview {
  enabled: boolean;
  extract: boolean;
  graph: ContextGraph;
  /** Pending only — the review queue. */
  proposals: ContextProposal[];
  stats: ContextStats;
}

/** What `context_bootstrap` recorded from the project's primary repo. */
export interface ContextBootstrap {
  /** The commit the repository was read at. */
  commit: string;
  /** Modules the tree describes. */
  modules: number;
  /** Slugs recorded by this run; empty when every module was already there. */
  created: string[];
  /** The canned first message of a mapping session. */
  mapping_task: string;
}

export type ProposalVerdict = "accept" | "dismiss";

/** `context:changed`: a write landed on the project's context; reload. */
export interface ContextChangedEvent {
  project_id: string;
}
