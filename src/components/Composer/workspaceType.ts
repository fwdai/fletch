// The composer's workspace type for a new agent: this Mac's sandbox or an
// isolated VM. The host computes the options (`available_workspace_types`);
// these helpers only decide whether the control is offered, what it shows and
// what the spawn carries, so the rule is the host's and the control cannot
// disagree with it.

import type { WorkspaceKind, WorkspaceTypeOption } from "@/api";
import { boxesUnlockedFor } from "@/util/boxesRollout";

/** Whether a new agent is offered a workspace type at all: boxes are rolled
 *  out to this user, and the agent starts on this Mac — a paired host's spawn
 *  carries no workspace type yet. The composer shows the control and the
 *  spawn sends the pick on this one rule. */
export function workspaceTypeOffered(admin: boolean, onThisMac: boolean): boolean {
  return boxesUnlockedFor(admin) && onThisMac;
}

/** What the spawn carries: the user's explicit pick, only while the control
 *  is offered. A draft the user never touched carries nothing, and the host
 *  decides. */
export function workspaceTypeToSend(
  choice: WorkspaceKind | undefined,
  offered: boolean,
): WorkspaceKind | undefined {
  return offered ? choice : undefined;
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
