// Which host an agent of a project can be started on: the rules of the
// desktop's "Runs on" picker (src/components/Composer/RunTargetPicker/
// options.ts) for a phone, which has no projects of its own. A host that has the
// project, as its home or a copy, runs it as it is. One that lacks it is given
// a copy first, carried across by the phone: a host that has the project
// describes it (`project_definition`) and the one that lacks it makes sure of
// it (`ensure_project`). Only connected hosts are weighed, as only they are
// offered.

import { GATES, requiredOps } from "@desktop/store/gates";
import { type HostProtocol, hostSupports } from "../remote";
import type { ProjectEntry } from "./projects";

/** A connected paired host, as the rules weigh it. */
export interface RunsOnHost {
  hostKey: string;
  name: string;
  /** What it said it answers (`HostState.protocol`). */
  protocol: HostProtocol | null;
}

const has = (entry: ProjectEntry, hostKey: string) =>
  entry.hosts.some((h) => h.hostKey === hostKey);

/** The host to ask for `entry`'s project as another host needs it
 *  (`project_definition`): of the `hosts` that have it and answer the op, the
 *  one that speaks for it, else the first. */
export function definitionSource(
  entry: ProjectEntry,
  hosts: readonly RunsOnHost[],
): RunsOnHost | undefined {
  const able = hosts.filter(
    (h) => has(entry, h.hostKey) && hostSupports(h.protocol, "project_definition"),
  );
  return able.find((h) => h.hostKey === entry.hostKey) ?? able[0];
}

/** Why an agent of `entry`'s project can't be started on `target`, or null
 *  when it can. A host that has the project always can, as the draft's own
 *  host can on the desktop. One that lacks it takes a copy, so it has to answer
 *  `ensure_project` (the desktop's `projectCopy` gate, and its wording) and one
 *  of `hosts` that has the project has to describe it. Whether every repository
 *  can be cloned is known only once it is described (`travelReason`), so that
 *  is asked when the copy is made. */
export function runsOnReason(
  entry: ProjectEntry,
  target: RunsOnHost,
  hosts: readonly RunsOnHost[],
): string | null {
  if (has(entry, target.hostKey)) return null;
  if (!requiredOps("projectCopy").every((op) => hostSupports(target.protocol, op))) {
    return GATES.projectCopy.reason;
  }
  if (definitionSource(entry, hosts)) return null;
  const holder =
    hosts.find((h) => h.hostKey === entry.hostKey) ?? hosts.find((h) => has(entry, h.hostKey));
  return holder
    ? `${holder.name} is too old to share this project. Update it.`
    : "No connected host has this project.";
}
