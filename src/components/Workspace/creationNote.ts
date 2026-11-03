import type { WorkspaceTypeOption } from "@/api";
import { workspaceTypeLabel } from "@/components/Composer/workspaceType";

/** What the new-workspace page says the spawn will do, around the
 *  workspace's path (`before` <path> `after`), or why it can't. */
export type CreationNote =
  | { kind: "path"; before: string; after: string }
  | { kind: "blocked"; text: string };

/** The sentence for the sandbox the spawn will send: `option` is the one the
 *  sandbox pill declares (`declaredWorkspaceType`), null while there is none
 *  to declare (no answer from the host yet, or a paired host, whose spawn
 *  carries no type), where the host decides and only the general shape is
 *  true. A VM keeps the checkout on its own disk at the same path (its `HOME`
 *  is the user's), so the path holds for both; only a macOS sandbox
 *  workspace leaves a checkout on this Mac. */
export function creationNote(option: WorkspaceTypeOption | null, repoCount: number): CreationNote {
  const many = repoCount > 1;
  const all = `all ${repoCount} repositories`;
  if (option && !option.available) {
    const why = option.reason?.trim().replace(/\.$/, "");
    const label = workspaceTypeLabel(option.kind);
    return { kind: "blocked", text: `The ${label} isn't available${why ? `: ${why}` : ""}.` };
  }
  if (option?.kind === "box") {
    return {
      kind: "path",
      before: `A Linux VM will be created for this workspace, with ${many ? `checkouts of ${all}` : "the checkout"} on the VM's own disk at `,
      after: ". Nothing is checked out on this Mac.",
    };
  }
  if (option?.kind === "host") {
    return {
      kind: "path",
      before: many
        ? `Checkouts of ${all} will be created on this Mac at `
        : "A checkout will be created on this Mac at ",
      after: ", and the agent will run there in the macOS sandbox.",
    };
  }
  return {
    kind: "path",
    before: many
      ? `Checkouts of ${all} and a sandbox will be created at `
      : "A checkout and sandbox will be created at ",
    after: "",
  };
}
