// Where a new workspace can run: the options of the run-target picker on the
// new-workspace row, and the words around them. Pure, and free of the store,
// so the spawn checks a pick by the same rule the picker greys it by.
//
// Each source contributes the places of one kind. Today that is the hosts this
// window knows, This Mac first and then each paired host by name; a provider
// that makes a host on demand would be a source of its own, appended to
// `SOURCES` with its own `kind`.

import type { ProjectDefinition, ProjectHome } from "@/api";
import { type EnvironmentEntry, type EnvironmentId, hostOrder } from "@/store/environments";
import { connectionLabel, gateReason, providerReason } from "@/store/gates";

export type RunTargetKind = "local" | "host";

export interface RunTargetOption {
  /** Unique among the options. */
  id: string;
  kind: RunTargetKind;
  /** The host the workspace runs on when this option is picked. */
  hostId: EnvironmentId;
  label: string;
  available: boolean;
  /** Why it can't be picked, for its tooltip; null while it can. */
  reason: string | null;
}

/** What the options are decided from. */
export interface RunTargetFacts {
  environments: Record<EnvironmentId, EnvironmentEntry>;
  /** The host whose project the draft is opened in. Always available: it is
   *  where the draft runs without a pick, the way it always has. */
  draftHost: EnvironmentId;
  /** The draft's provider, which has to be able to run where the workspace
   *  does. */
  provider: string;
  /** The draft's project as its host describes it, once read; null until
   *  then, which blocks nothing. */
  definition: ProjectDefinition | null;
}

/** Why the draft's project can't run on any device but its own, or null when
 *  it can: every repository has to be cloned there, so each needs a remote. */
export function travelReason(definition: ProjectDefinition | null): string | null {
  const stuck = definition?.repos.find((r) => r.origin === null);
  return stuck ? `${stuck.label || stuck.dir} has no remote. Publish it first.` : null;
}

/** Why a host other than the draft's own can't run its workspace, or null. */
function refusal(facts: RunTargetFacts, env: EnvironmentEntry): string | null {
  if (env.connection !== "connected") return `${env.name} · ${connectionLabel(env)}`;
  return (
    gateReason(env, "projectCopy") ??
    travelReason(facts.definition) ??
    providerReason(env, facts.provider)
  );
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

/** What a menu section of each kind is headed. */
export const RUN_TARGET_SECTIONS: Record<RunTargetKind, string> = {
  local: "This Mac",
  host: "Paired hosts",
};

/** The device a project's unpushed commits live on: its home, by the name
 *  this window knows it by, else the host the draft is on. */
export function homeName(
  environments: Record<EnvironmentId, EnvironmentEntry>,
  draftHost: EnvironmentId,
  home: ProjectHome | undefined,
): string {
  if (home) return environments[home.device]?.name ?? home.name;
  return environments[draftHost]?.name ?? "This Mac";
}

/** The note under the row while the workspace runs on a host other than the
 *  draft's own: the copy there is cloned from the remote. */
export function pushedNote(home: string): string {
  return `Starts from the branch as pushed. Commits not pushed from ${home} are not included.`;
}

/** What the composer says while the project is carried to `host`. */
export function preparingLabel(host: string): string {
  return `Preparing the project on ${host}…`;
}
