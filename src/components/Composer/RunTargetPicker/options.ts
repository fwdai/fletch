// Which machine a new workspace can run on: the options of the run-target
// picker on the composer's sheet, and the words around them. Pure, and free of
// the store, so the move checks a pick by the same rule the picker greys it
// by.
//
// Each source contributes the places of one kind. Today that is the hosts this
// window knows, This Mac first and then each paired host by name; a provider
// that makes a host on demand would be a source of its own, appended to
// `SOURCES` with its own `kind`.

import type { ProjectDefinition, ProjectHome } from "@/api";
import { type EnvironmentEntry, type EnvironmentId, hostOrder } from "@/store/environments";
import { connectionLabel, gateReason } from "@/store/gates";
import { travelReason } from "./copyWords";

export { preparingLabel, pushedNote, travelReason } from "./copyWords";

export type RunTargetKind = "local" | "host";

export interface RunTargetOption {
  /** Unique among the options. */
  id: string;
  kind: RunTargetKind;
  /** The host the workspace runs on when this option is picked. */
  hostId: EnvironmentId;
  label: string;
  available: boolean;
  /** Why it can't be picked, written under its name; null while it can. */
  reason: string | null;
}

/** What the options are decided from. */
export interface RunTargetFacts {
  environments: Record<EnvironmentId, EnvironmentEntry>;
  /** The host the draft is on, which is where it runs. Always available. */
  draftHost: EnvironmentId;
  /** The draft's project as its host describes it, once read; null until
   *  then, which blocks nothing. */
  definition: ProjectDefinition | null;
}

/** Why the draft can't move to a host other than its own, or null. Which
 *  agents that host can run is not asked here: once the draft is there, its
 *  composer offers that host's own, as for any draft opened on it. */
function refusal(facts: RunTargetFacts, env: EnvironmentEntry): string | null {
  if (env.connection !== "connected") return connectionLabel(env);
  return gateReason(env, "projectCopy") ?? travelReason(facts.definition);
}

/** This Mac and every paired host. */
function knownHosts(facts: RunTargetFacts): RunTargetOption[] {
  return hostOrder(facts.environments).map((id) => {
    const env = facts.environments[id];
    const reason = id === facts.draftHost ? null : refusal(facts, env);
    return {
      id,
      kind: env.kind === "local" ? "local" : "host",
      hostId: id,
      label: env.name,
      available: reason === null,
      reason,
    };
  });
}

const SOURCES: ((facts: RunTargetFacts) => RunTargetOption[])[] = [knownHosts];

/** Every place the draft's workspace could run, in the order the picker lists
 *  them. */
export function runTargetOptions(facts: RunTargetFacts): RunTargetOption[] {
  return SOURCES.flatMap((source) => source(facts));
}

/** What a menu section of each kind is headed. This Mac is one place, named
 *  by its own row, so its section has no heading. */
export const RUN_TARGET_SECTIONS: Record<RunTargetKind, string | null> = {
  local: null,
  host: "Paired hosts",
};

/** The device a copied project's unpushed commits live on: its home, by the
 *  name this window knows it by, else the one it gave itself. */
export function homeName(
  environments: Record<EnvironmentId, EnvironmentEntry>,
  home: ProjectHome,
): string {
  return environments[home.device]?.name ?? home.name;
}
