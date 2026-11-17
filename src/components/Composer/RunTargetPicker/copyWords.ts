// What running a project on a host other than its own takes, in words: why it
// can't, and what is said while and after its copy is made. Free of every
// store, so the phone says the same things (mobile/src/sheets/NewAgentSheet).

import type { ProjectDefinition } from "@/api";

/** Why the project can't run on any device but its own, or null when it can:
 *  every repository has to be cloned there, so each needs a remote. */
export function travelReason(definition: ProjectDefinition | null): string | null {
  const stuck = definition?.repos.find((r) => r.origin === null);
  return stuck ? `${stuck.label || stuck.dir} has no remote. Publish it first.` : null;
}

/** The note for a new workspace on a host that runs the project from a copy:
 *  the copy is cloned from the remote. */
export function pushedNote(home: string): string {
  return `Starts from the branch as pushed. Commits not pushed from ${home} are not included.`;
}

/** What is said while the project is being copied to `host`. */
export function preparingLabel(host: string): string {
  return `Preparing the project on ${host}…`;
}
