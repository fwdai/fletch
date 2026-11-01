// The composer's workspace type for a new agent: this Mac's sandbox or an
// isolated VM. The chip declares where the agent runs: the spawn carries
// exactly the kind the chip shows, the user's pick or the host's default
// alike, so the host lands the agent there or refuses with why, and the
// display cannot disagree with the outcome. The options are the host's
// (`available_workspace_types`), read for one set of inputs and never shown
// for others; while there is no such answer nothing is shown and nothing sent.

import type { WorkspaceKind, WorkspaceTypeOption } from "@/api";
import { boxesUnlockedFor } from "@/util/boxesRollout";

/** Whether a new agent is offered a workspace type at all: boxes are rolled
 *  out to this user, and the agent starts on this Mac — a paired host's spawn
 *  carries no workspace type yet. The composer shows the control and the
 *  spawn sends the pick on this one rule. */
export function workspaceTypeOffered(admin: boolean, onThisMac: boolean): boolean {
  return boxesUnlockedFor(admin) && onThisMac;
}

/** What a spawn with no choice gets, or null when neither option is it (the
 *  host pins a container engine in Settings). */
export function defaultWorkspaceType(options: WorkspaceTypeOption[]): WorkspaceKind | null {
  return options.find((o) => o.default)?.kind ?? null;
}

/** Whether the chip is drawn for these options: only when one of them is the
 *  default. With a container engine pinned in Settings neither is, and the two
 *  types cannot say "as in Settings", so the control stays away. */
export function workspaceTypeVisible(options: WorkspaceTypeOption[] | null): boolean {
  return options !== null && defaultWorkspaceType(options) !== null;
}

/** The option the chip shows: the user's explicit pick whenever the host
 *  lists it — even when it is the default, or no longer available (the spawn
 *  then fails with its reason rather than landing elsewhere) — else the
 *  default. */
export function shownWorkspaceType(
  options: WorkspaceTypeOption[],
  choice: WorkspaceKind | undefined,
): WorkspaceTypeOption | null {
  return (
    (choice && options.find((o) => o.kind === choice)) || options.find((o) => o.default) || null
  );
}

/** The words the composer uses for each type. */
export function workspaceTypeLabel(kind: WorkspaceKind): string {
  return kind === "box" ? "Isolated VM" : "This Mac";
}

/** The option the chip declares: the shown option while the host offers the
 *  control (one option is the default), else none. The chip draws it and the
 *  spawn sends its kind, so the two are one derivation. */
export function declaredWorkspaceType(
  options: WorkspaceTypeOption[] | null,
  choice: WorkspaceKind | undefined,
): WorkspaceTypeOption | null {
  return options && workspaceTypeVisible(options) ? shownWorkspaceType(options, choice) : null;
}

/** What the spawn carries: the kind the chip declares, the default included,
 *  while the control is offered and `options` is the host's answer for the
 *  draft's current inputs. Nothing otherwise (not offered, still loading, the
 *  host could not answer, or a container engine pinned): the host decides. */
export function workspaceTypeToSend(
  choice: WorkspaceKind | undefined,
  offered: boolean,
  options: WorkspaceTypeOption[] | null,
): WorkspaceKind | undefined {
  return offered ? declaredWorkspaceType(options, choice)?.kind : undefined;
}

/** The inputs an answer is for, as one key: the repo and the provider, and the
 *  two settings that move the default wholesale (the engine pinned in
 *  Settings, the boxes switch). An answer is shown, and sent, only for its own
 *  key. */
export function workspaceTypesKey(
  repoPath: string,
  provider: string,
  sandboxEngine: string,
  boxesEnabled: boolean,
): string {
  return [repoPath, provider, sandboxEngine, String(boxesEnabled)].join("\u0000");
}

/** One successful read: the options and the key of the inputs it was for. */
export interface WorkspaceTypeAnswer {
  key: string;
  options: WorkspaceTypeOption[];
}

/** The answer's options when it is for `key`, else null. */
export function answeredOptions(
  answer: WorkspaceTypeAnswer | null,
  key: string,
): WorkspaceTypeOption[] | null {
  return answer?.key === key ? answer.options : null;
}

/** Whether a read's response may land: it is the latest one started and its
 *  key is the inputs the picker on screen has now (`null` when none is). */
export function workspaceTypeResponseLands(
  request: { id: number; key: string },
  latestId: number,
  currentKey: string | null,
): boolean {
  return request.id === latestId && request.key === currentKey;
}

/** Whether the user's pick must be dropped: only when the host answered that
 *  the control is not offered (no option is the default). A pending or failed
 *  read is no such answer. */
export function workspaceTypePickDropped(options: WorkspaceTypeOption[] | null): boolean {
  return options !== null && !workspaceTypeVisible(options);
}

export const WORKSPACE_TYPES_UNREADABLE = "Couldn't load the workspace types. Click to retry.";

/** What the chip draws: nothing (no answer for the current inputs yet, or the
 *  host does not offer the control), a retry chip (no answer for these inputs
 *  and the last read failed; it shows the pick if there is one, and the menu
 *  stays shut), or the menu chip for the declared option. */
export type WorkspaceTypeChip =
  | null
  | { kind: "retry"; pick: WorkspaceKind | null; label: string; tip: string }
  | {
      kind: "menu";
      options: WorkspaceTypeOption[];
      shown: WorkspaceTypeOption;
      tip: string | undefined;
    };

export function workspaceTypeChip(
  options: WorkspaceTypeOption[] | null,
  failed: boolean,
  choice: WorkspaceKind | undefined,
): WorkspaceTypeChip {
  if (!options) {
    if (!failed) return null;
    return {
      kind: "retry",
      pick: choice ?? null,
      label: choice ? workspaceTypeLabel(choice) : "Workspace",
      tip: WORKSPACE_TYPES_UNREADABLE,
    };
  }
  const shown = declaredWorkspaceType(options, choice);
  if (!shown) return null;
  // An option shown but unavailable (a project pinned to VMs on a runtime
  // that cannot boot one) says why the launch will fail.
  const tip = shown.available ? "Where this workspace runs" : (shown.reason ?? undefined);
  return { kind: "menu", options, shown, tip };
}
