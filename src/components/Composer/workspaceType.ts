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

/** What the host has answered the picker so far: the options of its last
 *  successful read (null until there is one) and whether the latest read
 *  failed. A failure keeps the options already shown. */
export interface WorkspaceTypeAnswer {
  options: WorkspaceTypeOption[] | null;
  failed: boolean;
}

/** Before the first read lands: the picker draws nothing. */
export const UNANSWERED: WorkspaceTypeAnswer = { options: null, failed: false };

/** The answer after a read: its options when it succeeded, else the previous
 *  options with the failure noted. */
export function answerWith(
  prev: WorkspaceTypeAnswer,
  read: WorkspaceTypeOption[] | "failed",
): WorkspaceTypeAnswer {
  return read === "failed"
    ? { options: prev.options, failed: true }
    : { options: read, failed: false };
}

/** One read of the options: its sequence number and the inputs it asked for. */
export interface WorkspaceTypeRequest {
  id: number;
  repoPath: string;
  provider: string;
}

/** Whether a read's response may land: it is the latest one started and it
 *  asked for the inputs the picker has now, so an older repo's or provider's
 *  options never overwrite the current ones. */
export function workspaceTypeResponseLands(
  request: WorkspaceTypeRequest,
  latestId: number,
  inputs: { repoPath: string; provider: string },
): boolean {
  return (
    request.id === latestId &&
    request.repoPath === inputs.repoPath &&
    request.provider === inputs.provider
  );
}

/** Whether the user's pick must be dropped: only when the host answered that
 *  the control is not offered (no option is the default). A failed or pending
 *  read is no such answer. */
export function workspaceTypePickDropped(answer: WorkspaceTypeAnswer): boolean {
  return answer.options !== null && !workspaceTypeVisible(answer.options);
}

export const WORKSPACE_TYPES_UNREADABLE = "Couldn't load the workspace types. Click to retry.";

/** What the chip draws for an answer and the user's pick: nothing (first read
 *  pending, or the host does not offer the control), a retry chip (no read has
 *  succeeded and the last one failed; it shows the pick if there is one, and
 *  the menu stays shut), or the menu chip for the shown option. */
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
  answer: WorkspaceTypeAnswer,
  choice: WorkspaceKind | undefined,
): WorkspaceTypeChip {
  const { options } = answer;
  if (!options) {
    if (!answer.failed) return null;
    return {
      kind: "retry",
      pick: choice ?? null,
      label: choice ? workspaceTypeLabel(choice) : "Workspace",
      tip: WORKSPACE_TYPES_UNREADABLE,
    };
  }
  if (!workspaceTypeVisible(options)) return null;
  const shown = shownWorkspaceType(options, choice);
  if (!shown) return null;
  // An option shown but unavailable (a project pinned to VMs on a runtime
  // that cannot boot one) says why the launch will fail.
  const tip = shown.available ? "Where this workspace runs" : (shown.reason ?? undefined);
  return { kind: "menu", options, shown, tip };
}
