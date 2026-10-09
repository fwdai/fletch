// Pure helpers behind the Context tab: grouping, badge labels and the
// client-side history walk. No React, no API — unit-tested on their own.

import type {
  AssertionKind,
  AssertionStatus,
  Author,
  ContextAssertion,
  ContextEntity,
  ContextGraph,
  ContextProposal,
  ContradictionEdge,
  Domain,
  EntityKind,
  Provenance,
  Source,
} from "@/api";
import type { BadgeVariant } from "@/components/ui/Badge";

/** Project-settings keys the host reads (`context::ENABLED_KEY` /
 *  `context::EXTRACT_KEY`). Both opt-out: absent is on, `"false"` is off. */
export const ENABLED_KEY = "context.enabled";
export const EXTRACT_KEY = "context.extract";

export function flagOn(value: string | undefined): boolean {
  return value !== "false";
}

export const ENTITY_KINDS: EntityKind[] = [
  "vision",
  "goal",
  "capability",
  "feature",
  "module",
  "topic",
];
export const ASSERTION_KINDS: AssertionKind[] = ["decision", "constraint", "fact"];
export const DOMAINS: Domain[] = ["business", "architectural", "implementation"];

export const KIND_LABEL: Record<EntityKind, string> = {
  vision: "Vision",
  goal: "Goals",
  capability: "Capabilities",
  feature: "Features",
  module: "Modules",
  topic: "Topics",
};

export const ASSERTION_KIND_LABEL: Record<AssertionKind, string> = {
  decision: "Decisions",
  constraint: "Constraints",
  fact: "Facts",
};

/** Active entities, grouped in the vocabulary's order; empty kinds are left out. */
export function groupEntitiesByKind(
  entities: ContextEntity[],
): { kind: EntityKind; entities: ContextEntity[] }[] {
  return ENTITY_KINDS.map((kind) => ({
    kind,
    entities: entities
      .filter((e) => e.kind === kind && e.status === "active")
      .sort((a, b) => a.name.localeCompare(b.name)),
  })).filter((g) => g.entities.length > 0);
}

/** Case-insensitive match on slug, name or any alias; blank matches all. */
export function matchesSearch(entity: ContextEntity, query: string): boolean {
  const q = query.trim().toLowerCase();
  if (!q) return true;
  return (
    entity.slug.toLowerCase().includes(q) ||
    entity.name.toLowerCase().includes(q) ||
    entity.aliases.some((a) => a.toLowerCase().includes(q))
  );
}

/** Whether an assertion stands now — the host's answer (`graph.current`),
 *  the same rule compile applies, so the tab never disagrees with what an
 *  agent is served. */
export function isCurrent(graph: ContextGraph, assertion: ContextAssertion): boolean {
  return graph.current.includes(assertion.id);
}

/** The assertions shown under one entity: everything current, plus retracted
 *  and abandoned ones that nothing replaced — badged, so the user can see what
 *  was ruled out. A superseded one shows up under its successor's history. */
export function headsAbout(graph: ContextGraph, entityId: string): ContextAssertion[] {
  return graph.assertions.filter(
    (a) => a.about.includes(entityId) && (isCurrent(graph, a) || !a.superseded_by),
  );
}

export interface AssertionGroup {
  kind: AssertionKind;
  domains: { domain: Domain; assertions: ContextAssertion[] }[];
}

/** Group by kind, then domain; within a domain adopted before rejected, then
 *  newest first. Empty groups are left out. */
export function groupAssertions(assertions: ContextAssertion[]): AssertionGroup[] {
  const byStance = (a: ContextAssertion, b: ContextAssertion) =>
    a.stance === b.stance ? b.recorded_at - a.recorded_at : a.stance === "adopted" ? -1 : 1;
  return ASSERTION_KINDS.map((kind) => ({
    kind,
    domains: DOMAINS.map((domain) => ({
      domain,
      assertions: assertions.filter((a) => a.kind === kind && a.domain === domain).sort(byStance),
    })).filter((d) => d.assertions.length > 0),
  })).filter((g) => g.domains.length > 0);
}

export function statusVariant(status: AssertionStatus): BadgeVariant {
  switch (status) {
    case "confirmed":
      return "ok";
    case "provisional":
      return "warn";
    case "retracted":
      return "err";
    case "abandoned":
      return "neutral";
  }
}

/** "user", "agent fuji", "extractor fuji", "ingester". */
export function authorLabel(author: Author): string {
  switch (author.kind) {
    case "user":
      return "user";
    case "ingester":
      return "ingester";
    case "agent":
    case "extractor":
      return author.agent_id ? `${author.kind} ${author.agent_id}` : author.kind;
  }
}

const SOURCE_LABEL: Record<Source["kind"], string> = {
  user_turn: "user turn",
  agent_turn: "agent turn",
  pr: "PR",
  review_thread: "review thread",
  roadmap: "roadmap",
  workflow: "workflow",
  ui: "UI",
  repo: "repo at",
};

/** "from user turn", "from PR #12" (the reference is stored as written), "from UI". */
export function sourceLabel(source: Source): string {
  const name = SOURCE_LABEL[source.kind];
  if (!source.reference) return `from ${name}`;
  return `from ${name} ${source.reference}`;
}

/** "agent fuji · quorum" — the author, then the checkout when the record
 *  names one. */
export function provenanceLabel(author: Author, provenance: Provenance): string {
  const who = authorLabel(author);
  return provenance.repo ? `${who} · ${provenance.repo}` : who;
}

/** One `contradicts` edge seen from one of its sides. `other` is missing when
 *  the graph no longer carries the counterpart. */
export interface Tension {
  edge: ContradictionEdge;
  otherId: string;
  other?: ContextAssertion;
}

function tensionsTouching(graph: ContextGraph, assertionId: string): Tension[] {
  return graph.contradictions
    .filter((e) => e.a === assertionId || e.b === assertionId)
    .map((edge) => {
      const otherId = edge.a === assertionId ? edge.b : edge.a;
      return { edge, otherId, other: graph.assertions.find((x) => x.id === otherId) };
    });
}

/** The open tensions behind the "contradicted" marker: unresolved edges, in
 *  either orientation, whose other side still stands (`graph.current`). */
export function openTensions(graph: ContextGraph, assertionId: string): Tension[] {
  return tensionsTouching(graph, assertionId).filter(
    (t) => !t.edge.resolution && !!t.other && isCurrent(graph, t.other),
  );
}

/** The tensions a ruling closed, for the history drawer. */
export function resolvedTensions(graph: ContextGraph, assertionId: string): Tension[] {
  return tensionsTouching(graph, assertionId).filter((t) => !!t.edge.resolution);
}

/** "resolved: <reasoning>, by <author>, <date>". */
export function resolutionLabel(edge: ContradictionEdge): string {
  const r = edge.resolution;
  if (!r) return "";
  const date = new Date(r.at).toLocaleDateString(undefined, {
    year: "numeric",
    month: "short",
    day: "numeric",
  });
  return `resolved: ${r.reasoning}, by ${authorLabel(r.by)}, ${date}`;
}

/** Of a proposal's pending subjects, the slugs no active entity carries yet
 *  (case-insensitive) — each one blocks Accept until its entity lands. */
export function unacceptedPending(graph: ContextGraph, slugs: string[]): string[] {
  const active = new Set(
    graph.entities.filter((e) => e.status === "active").map((e) => e.slug.toLowerCase()),
  );
  return slugs.filter((s) => !active.has(s.toLowerCase()));
}

/** The supersession chain behind an assertion, newest predecessor first,
 *  walked client-side over `supersedes` ids. Stops on a missing id or a cycle. */
export function historyOf(graph: ContextGraph, assertionId: string): ContextAssertion[] {
  const chain: ContextAssertion[] = [];
  const seen = new Set<string>([assertionId]);
  let current = graph.assertions.find((a) => a.id === assertionId);
  while (current?.supersedes) {
    const prev = graph.assertions.find((a) => a.id === current?.supersedes?.id);
    if (!prev || seen.has(prev.id)) break;
    seen.add(prev.id);
    chain.push(prev);
    current = prev;
  }
  return chain;
}

/** A comma-separated field into trimmed, non-empty items. */
export function splitList(text: string): string[] {
  return text
    .split(",")
    .map((s) => s.trim())
    .filter(Boolean);
}

/** A slug suggestion from a name: lowercase, hyphens, nothing else. */
export function slugify(name: string): string {
  return name
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "");
}

/** An entity's display name by id, falling back to the raw id for one the
 *  graph no longer carries. */
export function entityName(graph: ContextGraph, id: string): string {
  return graph.entities.find((e) => e.id === id)?.name ?? id;
}

/** The distinct turns a proposal's quotes came from. A repeat of a pending
 *  proposal lands as more quotes on it, but quotes from one turn are one
 *  sighting, not several. */
function turnCount(proposal: ContextProposal): number {
  return new Set(proposal.evidence.flatMap((ev) => (ev.turn_id ? [ev.turn_id] : []))).size;
}

/** How well a proposal is corroborated: its distinct turns, or its quotes
 *  when none carries a turn. */
export function corroboration(proposal: ContextProposal): number {
  return turnCount(proposal) || proposal.evidence.length;
}

/** The corroboration badge ("3 turns", "2 quotes"), or null for one quote
 *  or none. */
export function corroborationLabel(proposal: ContextProposal): string | null {
  const turns = turnCount(proposal);
  if (turns > 1) return `${turns} turns`;
  return proposal.evidence.length > 1 ? `${proposal.evidence.length} quotes` : null;
}

/** The review queue's order: entities first (assertions about them wait on
 *  their acceptance), then the best corroborated, then the oldest. */
export function sortForReview(proposals: ContextProposal[]): ContextProposal[] {
  const rank = (p: ContextProposal) => (p.payload.type === "entity" ? 0 : 1);
  return [...proposals].sort(
    (a, b) =>
      rank(a) - rank(b) || corroboration(b) - corroboration(a) || a.created_at - b.created_at,
  );
}

/** One line for a proposal's payload: what would land if accepted. */
export function summarizeProposal(proposal: ContextProposal): string {
  const p = proposal.payload;
  if (p.type === "entity") {
    const verb = p.input.id ? "Revise" : "New";
    return `${verb} ${p.input.kind} "${p.input.name}" (${p.input.slug})`;
  }
  return `${p.input.stance === "rejected" ? "Rejected " : ""}${p.input.kind} (${p.input.domain}): ${p.input.statement}`;
}
