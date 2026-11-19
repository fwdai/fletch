import type { WorkspaceTypeOption } from "@/api";
import { workspaceTypeLabel } from "@/components/Composer/workspaceType";
import { cloudMachinePhrase } from "@/util/sandboxNames";
import { isCloudKind } from "@/util/workspaceKind";

/** The new-workspace page's one-line note: the words before the workspace's
 *  path, or why it can't be created. */
export type CreationNote = { kind: "path"; before: string } | { kind: "blocked"; text: string };

/** The note for the sandbox the spawn will send: `option` is the one the
 *  sandbox pill declares (`declaredWorkspaceType`), null while there is none
 *  to declare (no answer from the host yet, or a paired host), where only the
 *  general shape is true. A VM, on this Mac or at a cloud provider, keeps the
 *  checkout at the same path on its own disk, so the path holds for all. */
export function creationNote(option: WorkspaceTypeOption | null, repoCount: number): CreationNote {
  if (option && !option.available) {
    const why = option.reason?.trim().replace(/\.$/, "");
    return {
      kind: "blocked",
      text: `${workspaceTypeLabel(option)} isn't available${why ? `: ${why}` : ""}`,
    };
  }
  const what = repoCount > 1 ? `Checkouts of all ${repoCount} repositories` : "A checkout";
  if (option && isCloudKind(option.kind)) {
    const where = cloudMachinePhrase(option.label);
    return { kind: "path", before: `${what} will be created on ${where}, at ` };
  }
  if (option?.kind === "box")
    return { kind: "path", before: `${what} will be created in a Linux VM at ` };
  if (option?.kind === "host")
    return { kind: "path", before: `${what} will be created on this Mac at ` };
  return {
    kind: "path",
    before:
      repoCount > 1
        ? `Checkouts of all ${repoCount} repositories and a sandbox will be created at `
        : "A checkout and sandbox will be created at ",
  };
}
