// builder/promote.ts — seed the workflow builder from a promoted ad-hoc session
// (spec §14.1 / PR "close the cockpit ↔ workflow loop"). Pure: maps a
// `PromoteSeed` to a fresh `EditorState` whose single step runs the session's
// agent on the session's brief. The run's fork point (the session's HEAD) rides
// alongside as a launch parameter — not part of the definition — so the promoted
// workflow stays reusable while this one launch forks where the session left off.

import type { CustomAgent } from "../../storage/customAgents";
import type { PromoteSeed } from "../../store/types";
import { blankEditor, type EditorState, ensureAlias } from "./model";

/** First non-empty line of the brief, trimmed to a workflow-name length. */
function nameFromBrief(brief: string): string {
  const line = brief.split("\n").find((l) => l.trim()) ?? "";
  const head = line.trim();
  return head.length > 60 ? `${head.slice(0, 59)}…` : head;
}

/** Build editor state for a promoted session: one step, assigned to the
 *  session's agent (a reused/synthesized alias), goaled with the brief. The
 *  workflow is named from the brief so it's launchable without re-typing, and
 *  finalize stays off — promotion never lifts the run's publish boundary. */
export function seedEditorFromPromotion(
  seed: PromoteSeed,
  customAgents: CustomAgent[],
  seedIndex: number,
  agentName: string,
): EditorState {
  const base = blankEditor(seedIndex);
  const { agents, alias } = ensureAlias(base.agents, seed.agentPick, customAgents);
  const name = nameFromBrief(seed.task) || `Promoted from ${agentName}`;
  const description = `Promoted from the “${agentName}” session`;
  const step = base.blocks[0];
  if (step.kind !== "step") return { ...base, name, description, agents };
  return {
    ...base,
    name,
    description,
    agents,
    blocks: [{ ...step, agent: alias, goal: seed.task }],
  };
}
