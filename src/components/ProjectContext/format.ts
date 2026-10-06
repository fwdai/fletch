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
  Domain,
  EntityKind,
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

/** The current assertions about one entity: heads only (a superseded one
 *  shows up under its successor's history). Retracted and abandoned heads stay,
 *  badged, so the user can see what was ruled out. */
export function headsAbout(graph: ContextGraph, entityId: string): ContextAssertion[] {
  return graph.assertions.filter((a) => a.about.includes(entityId) && !a.superseded_by);
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
  brief: "brief",
  workflow: "workflow",
  ui: "UI",
};

/** "from user turn", "from PR #12" (the reference is stored as written), "from UI". */
export function sourceLabel(source: Source): string {
  const name = SOURCE_LABEL[source.kind];
  if (!source.reference) return `from ${name}`;
  return `from ${name} ${source.reference}`;
}

/** Live heads this assertion is in tension with — the "contradicted" marker.
 *  A retracted, abandoned or superseded counterpart no longer counts. */
export function contradictedBy(
  graph: ContextGraph,
  assertion: ContextAssertion,
): ContextAssertion[] {
  return assertion.contradicts
    .map((id) => graph.assertions.find((a) => a.id === id))
    .filter((a): a is ContextAssertion => !!a && !a.superseded_by && isLive(a.status));
}

function isLive(status: AssertionStatus): boolean {
  return status !== "retracted" && status !== "abandoned";
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

/** One line for a proposal's payload: what would land if accepted. */
export function summarizeProposal(proposal: ContextProposal): string {
  const p = proposal.payload;
  if (p.type === "entity") {
    const verb = p.input.id ? "Revise" : "New";
    return `${verb} ${p.input.kind} "${p.input.name}" (${p.input.slug})`;
  }
  return `${p.input.stance === "rejected" ? "Rejected " : ""}${p.input.kind} (${p.input.domain}): ${p.input.statement}`;
}
