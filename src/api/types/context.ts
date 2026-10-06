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
  | "brief"
  | "workflow"
  | "ui";

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

export interface ContextGraph {
  project_id: string;
  entities: ContextEntity[];
  assertions: ContextAssertion[];
  relations: ContextRelation[];
}

export interface CompileQuery {
  entities: string[];
  query?: string;
  paths: string[];
  include_history: boolean;
  as_of?: number;
  budget_chars: number;
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
  | { type: "assertion"; input: AssertionInput; stamp: Stamp; relation: ProposedRelation };

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

export type ProposalVerdict = "accept" | "dismiss";

/** `context:changed`: a write landed on the project's context; reload. */
export interface ContextChangedEvent {
  project_id: string;
}
